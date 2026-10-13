use anyhow::Result;
use orion_types::{JarvisConfig, VoiceConfig, AudioChunk, Transcription, TranscriptionSegment, TTSRequest, TTSResponse};
use tracing::{info, debug, warn, error};
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::process::Command;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, StreamConfig, Device};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use bytes::BytesMut;
use std::path::PathBuf;
use dirs;
use serde_json;
use orion_types::MessageBus;

pub struct VoicePipeline {
    config: VoiceConfig,
    bus: Arc<MessageBus>,
    whisper_binary: PathBuf,
    piper_binary: PathBuf,
    piper_model: PathBuf,
    audio_tx: Option<mpsc::Sender<AudioChunk>>,
    is_listening: Arc<tokio::sync::RwLock<bool>>,
}

impl VoicePipeline {
    pub async fn new(config: &VoiceConfig, bus: Arc<MessageBus>) -> Result<Self> {
        info!("Initializing Voice Pipeline (binary mode)");

        let whisper_binary = Self::find_binary("whisper-cli")
            .or_else(|| Self::find_binary_in_vendor("whisper.cpp/build/bin/whisper-cli"))
            .ok_or_else(|| anyhow::anyhow!("whisper-cli not found"))?;

        let piper_binary = Self::find_binary("piper")
            .or_else(|| Self::find_binary_in_vendor("piper/build/piper"))
            .ok_or_else(|| anyhow::anyhow!("piper not found"))?;

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let models_dir = home.join(".orion").join("models");
        let piper_model = models_dir.join("en_US-lessac-medium.onnx");

        info!("Using whisper: {}", whisper_binary.display());
        info!("Using piper: {}", piper_binary.display());
        info!("Using piper model: {}", piper_model.display());

        Ok(Self {
            config: config.clone(),
            bus,
            whisper_binary,
            piper_binary,
            piper_model,
            audio_tx: None,
            is_listening: Arc::new(tokio::sync::RwLock::new(false)),
        })
    }

    fn find_binary(name: &str) -> Option<PathBuf> {
        which::which(name).ok()
    }

    fn find_binary_in_vendor(relative: &str) -> Option<PathBuf> {
        let path = PathBuf::from("/home/unk-/Orion/ORION/vendor").join(relative);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting voice pipeline");

        let (audio_tx, mut audio_rx) = mpsc::channel::<AudioChunk>(100);
        self.audio_tx = Some(audio_tx);

        // Start audio capture
        let capture_handle = self.start_audio_capture().await?;

        // Start processing loop
        let process_handle = self.start_processing_loop(audio_rx).await?;

        // Subscribe to bus commands
        let cmd_handle = self.subscribe_commands().await?;

        // Wait for any handle to finish (error or shutdown)
        tokio::select! {
            res = capture_handle => res??,
            res = process_handle => res??,
            res = cmd_handle => res??,
        }

        Ok(())
    }

    async fn start_audio_capture(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let config = self.config.clone();
        let audio_tx = self.audio_tx.clone().unwrap();
        let is_listening = self.is_listening.clone();

        // cpal streams hold raw pointers (not Send) - run capture on a dedicated OS thread
        // and bridge chunks into the async world via tokio mpsc.
        let (bridge_tx, mut bridge_rx) = mpsc::channel::<AudioChunk>(1000);

        std::thread::Builder::new()
            .name("orion-audio-capture".into())
            .spawn(move || {
                let host = cpal::default_host();
                let device = match host.default_input_device() {
                    Some(d) => d,
                    None => {
                        error!("No input device available");
                        return;
                    }
                };

                let mut supported_config = match device.default_input_config() {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to get default input config: {}", e);
                        return;
                    }
                };

                if supported_config.sample_rate().0 != config.sample_rate {
                    if let Some(c) = device.supported_input_configs().ok().and_then(|mut configs| {
                        configs
                            .find(|c| c.min_sample_rate().0 <= config.sample_rate
                                && c.max_sample_rate().0 >= config.sample_rate
                                && c.channels() == 1)
                    }) {
                        supported_config = c.with_sample_rate(SampleRate(config.sample_rate));
                    } else {
                        error!("Unsupported sample rate: {}", config.sample_rate);
                        return;
                    }
                }

                let stream_config: StreamConfig = supported_config.into();

                let err_fn = |err| error!("Audio stream error: {}", err);

                let sample_rate = config.sample_rate;
                let stream = device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if *is_listening.blocking_read() {
                            let chunk = AudioChunk {
                                samples: data.to_vec(),
                                sample_rate,
                                channels: 1,
                                timestamp_ms: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis() as u64,
                            };
                            let _ = bridge_tx.blocking_send(chunk);
                        }
                    },
                    err_fn,
                    None,
                );

                match stream {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            error!("Failed to start audio stream: {}", e);
                            return;
                        }
                        info!("Audio capture started");
                        // Park this thread; the stream callback keeps running.
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(3600));
                        }
                    }
                    Err(e) => {
                        error!("Failed to build input stream: {}", e);
                    }
                }
            })?;

        let handle = tokio::spawn(async move {
            while let Some(chunk) = bridge_rx.recv().await {
                if audio_tx.send(chunk).await.is_err() {
                    break;
                }
            }
            Ok(())
        });

        Ok(handle)
    }

    async fn start_processing_loop(&self, mut audio_rx: mpsc::Receiver<AudioChunk>) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let config = self.config.clone();
        let bus = self.bus.clone();
        let whisper_binary = self.whisper_binary.clone();
        let piper_model = self.piper_model.clone();
        let is_listening = self.is_listening.clone();
        let wake_word = config.wake_word.clone();

        let handle = tokio::spawn(async move {
            let mut audio_buffer = Vec::new();
            let mut silence_frames = 0;
            let max_silence_frames = 30; // ~1.5s at 20ms frames
            let mut wake_detected = false;

            while let Some(chunk) = audio_rx.recv().await {
                // Simple VAD based on RMS energy
                let rms = Self::compute_rms(&chunk.samples);
                let is_speech = rms > 0.01; // Threshold

                if is_speech {
                    silence_frames = 0;
                    audio_buffer.extend_from_slice(&chunk.samples);

                    // Check for wake word if not already detected
                    if !wake_detected && audio_buffer.len() >= config.sample_rate as usize {
                        if let Ok(transcription) = Self::transcribe_buffer(&whisper_binary, &audio_buffer, config.sample_rate).await {
                            if transcription.text.to_lowercase().contains(&wake_word.to_lowercase()) {
                                wake_detected = true;
                                info!("Wake word detected: {}", wake_word);
                                let _ = bus.publish("orion.wake", &serde_json::json!({
                                    "timestamp": chunk.timestamp_ms
                                })).await;
                            }
                        }
                    }
                } else {
                    silence_frames += 1;

                    if wake_detected && silence_frames >= max_silence_frames && !audio_buffer.is_empty() {
                        // End of utterance - transcribe full buffer
                        if let Ok(transcription) = Self::transcribe_buffer(&whisper_binary, &audio_buffer, config.sample_rate).await {
                            if !transcription.text.trim().is_empty() {
                                info!("Transcribed: {}", transcription.text);
                                let _ = bus.publish("orion.transcription", &serde_json::json!({
                                    "text": transcription.text,
                                    "confidence": transcription.confidence,
                                    "language": transcription.language,
                                    "timestamp": chunk.timestamp_ms
                                })).await;
                            }
                        }

                        // Reset for next utterance
                        audio_buffer.clear();
                        silence_frames = 0;
                        wake_detected = false;
                    }
                }

                // Prevent buffer overflow
                if audio_buffer.len() > config.sample_rate as usize * 30 { // 30 seconds max
                    audio_buffer.drain(0..config.sample_rate as usize * 10);
                }
            }

            Ok(())
        });

        Ok(handle)
    }

    fn compute_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    async fn transcribe_buffer(
        whisper_binary: &PathBuf,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<Transcription> {
        // Write samples to temporary WAV file
        let temp_wav = std::env::temp_dir().join(format!("orion_stt_{}.wav", Self::gen_temp_id()));
        Self::write_wav(&temp_wav, samples, sample_rate)?;

        // Run whisper-cli
        let output = Command::new(whisper_binary)
            .args([
                "-m", "/home/unk-/.orion/models/ggml-large-v3.bin",
                "-f", temp_wav.to_str().unwrap(),
                "--no-timestamps",
                "-l", "en",
                "-otxt",
            ])
            .output()
            .await?;

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_wav);

        if !output.status.success() {
            return Err(anyhow::anyhow!("whisper-cli failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

        Ok(Transcription {
            text,
            confidence: 0.9, // whisper-cli doesn't output confidence easily
            language: "en".to_string(),
            segments: vec![],
        })
    }

    fn write_wav(path: &PathBuf, samples: &[f32], sample_rate: u32) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)?;
        for sample in samples {
            let s = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            writer.write_sample(s)?;
        }
        writer.finalize()?;
        Ok(())
    }

    async fn subscribe_commands(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let bus = self.bus.clone();
        let piper_binary = self.piper_binary.clone();
        let piper_model = self.piper_model.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            let mut sub = bus.subscribe("orion.tts").await?;

            while let Some(msg) = sub.next().await {
                if let Ok(req) = serde_json::from_slice::<TTSRequest>(&msg.payload) {
                    match Self::synthesize(&piper_binary, &piper_model, &req).await {
                        Ok(response) => {
                            let _ = bus.publish("orion.tts.response", &response).await;
                        }
                        Err(e) => {
                            error!("TTS error: {}", e);
                        }
                    }
                }
            }

            Ok(())
        });

        Ok(handle)
    }

    async fn synthesize(piper_binary: &PathBuf, piper_model: &PathBuf, req: &TTSRequest) -> Result<TTSResponse> {
        let temp_wav = std::env::temp_dir().join(format!("orion_tts_{}.wav", Self::gen_temp_id()));

        let mut cmd = Command::new(piper_binary);
        cmd.args([
            "--model", piper_model.to_str().unwrap(),
            "--output_file", temp_wav.to_str().unwrap(),
        ]);

        // Use stdin for text input
        let mut child = cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(req.text.as_bytes()).await?;
            stdin.flush().await?;
            drop(stdin);
        }

        let output = child.wait_with_output().await?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("piper failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        // Read the generated WAV
        let mut reader = hound::WavReader::open(&temp_wav)?;
        let spec = reader.spec();
        let samples: Vec<f32> = reader.samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32767.0)
            .collect();

        // Resample if needed
        let final_samples = if spec.sample_rate != req.sample_rate {
            Self::resample(&samples, spec.sample_rate, req.sample_rate)?
        } else {
            samples
        };

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_wav);

        Ok(TTSResponse {
            audio: AudioChunk {
                samples: final_samples,
                sample_rate: req.sample_rate,
                channels: 1,
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            },
            phonemes: None,
        })
    }

    fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
        if from_rate == to_rate {
            return Ok(input.to_vec());
        }

        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let mut resampler = SincFixedIn::<f32>::new(
            to_rate as f64 / from_rate as f64,
            2.0,
            params,
            input.len(),
            1,
        )?;

        // rubato 0.15 API: process returns Vec<Vec<f32>> (channels)
        let output_channels = resampler.process(&[input], None)?;
        let output = output_channels.into_iter().next().unwrap_or_default();

        Ok(output)
    }

    fn gen_temp_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        format!("{:x}", nanos)
    }
}