use anyhow::Result;
use orion_types::{JarvisConfig, PlanningConfig, PlanRequest, PlanResponse, PlanStep, ToolCall, ToolResult, MessageBus};
use tracing::{info, debug, warn, error};
use tokio::sync::mpsc;
use std::sync::Arc;
use std::path::PathBuf;
use tokenizers::Tokenizer;
use async_nats::Client as NatsClient;
use serde_json;
use reqwest;

pub struct PlanningCore {
    config: PlanningConfig,
    bus: Arc<MessageBus>,
    tokenizer: Option<Tokenizer>,
    nats: Option<NatsClient>,
}

impl PlanningCore {
    pub async fn new(config: &PlanningConfig, bus: Arc<MessageBus>) -> Result<Self> {
        info!("Initializing Planning Core");

        // Load tokenizer for prompt formatting
        let tokenizer = Self::load_tokenizer(&config.llm_model).await.ok();

        let nats = async_nats::connect("nats://127.0.0.1:4222").await.ok();

        Ok(Self {
            config: config.clone(),
            bus,
            tokenizer,
            nats,
        })
    }

    async fn load_tokenizer(model_name: &str) -> Result<Tokenizer> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let models_dir = home.join(".orion").join("models");
        let model_path = models_dir.join(model_name);

        Tokenizer::from_file(model_path.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("Tokenizer load failed: {}", e))
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting planning core");

        let cmd_handle = self.subscribe_commands().await?;

        cmd_handle.await??;

        Ok(())
    }

    async fn subscribe_commands(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let bus = self.bus.clone();
        let nats = self.nats.clone();
        let config = self.config.clone();
        let tokenizer = self.tokenizer.clone();

        let handle = tokio::spawn(async move {
            let mut sub = bus.subscribe("orion.planning.plan").await?;

            while let Some(msg) = sub.next().await {
                if let Ok(request) = serde_json::from_slice::<PlanRequest>(&msg.payload) {
                    let response = Self::plan(&request, &config, &tokenizer).await;
                    let _ = bus.publish("orion.planning.plan_response", &response).await;
                }
            }

            Ok(())
        });

        Ok(handle)
    }

    async fn plan(
        request: &PlanRequest,
        config: &PlanningConfig,
        tokenizer: &Option<Tokenizer>,
    ) -> PlanResponse {
        // Build planning prompt
        let prompt = Self::build_planning_prompt(request);

        let (steps, reasoning) = if let Some(tokenizer) = tokenizer {
            match Self::generate_plan(tokenizer, &prompt, config).await {
                Ok((steps, reasoning)) => (steps, reasoning),
                Err(e) => {
                    warn!("LLM planning failed: {}, using fallback", e);
                    Self::fallback_plan(request)
                }
            }
        } else {
            Self::fallback_plan(request)
        };

        let estimated_duration_ms = steps.iter().map(|_| 5000).sum(); // Rough estimate

        PlanResponse {
            steps,
            reasoning,
            estimated_duration_ms,
        }
    }

    fn build_planning_prompt(request: &PlanRequest) -> String {
        let tools_json = serde_json::to_string_pretty(&request.available_tools).unwrap_or_default();
        let context_json = serde_json::to_string_pretty(&request.context).unwrap_or_default();
        let constraints_json = serde_json::to_string_pretty(&request.constraints).unwrap_or_default();

        format!(r#"You are ORION, an autonomous AI assistant. Create a step-by-step plan to achieve the goal.

GOAL: {}

CONTEXT:
{}

CONSTRAINTS:
{}

AVAILABLE TOOLS:
{}

Respond with a JSON object containing:
- steps: array of {{\"tool\": \"tool_name\", \"args\": {{...}}, \"depends_on\": [step_indices], \"description\": \"what this step does\"}}
- reasoning: brief explanation of the plan

Only use tools from the available list. Keep steps minimal and focused."#, 
            request.goal, context_json, constraints_json, tools_json
        )
    }

    async fn generate_plan(
        tokenizer: &Tokenizer,
        prompt: &str,
        config: &PlanningConfig,
        request: &PlanRequest,
    ) -> Result<(Vec<PlanStep>, String)> {
        // Determine which model to use based on task_type
        let model = match request.task_type.as_deref() {
            Some("code") => std::env::var("NVIDIA_CODE_MODEL")
                .or_else(|_| std::env::var("NVIDIA_CHAT_MODEL"))
                .unwrap_or_else(|_| config.llm_model.clone()),
            Some("vision") => std::env::var("NVIDIA_VISION_MODEL")
                .or_else(|_| std::env::var("NVIDIA_CHAT_MODEL"))
                .unwrap_or_else(|_| config.llm_model.clone()),
            Some("embed") => std::env::var("NVIDIA_EMBED_MODEL")
                .or_else(|_| std::env::var("NVIDIA_CHAT_MODEL"))
                .unwrap_or_else(|_| config.llm_model.clone()),
            _ => {
                // Default to chat model
                std::env::var("NVIDIA_CHAT_MODEL").unwrap_or(config.llm_model.clone())
            }
        };
        // Try to get NVIDIA API key from environment
        let api_key = std::env::var("NVIDIA_API_KEY").ok();
        let api_base = std::env::var("NVIDIA_API_BASE").unwrap_or("https://integrate.api.nvidia.com/v1".to_string());

        if let Some(key) = api_key {
            // Call NVIDIA API
            let client = reqwest::Client::new();
            let response = client
                .post(&format!("{}/chat/completions", api_base))
                .header("Authorization", format!("Bearer {}", key))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [
                        {
                            "role": "system",
                            "content": "You are ORION, an autonomous AI assistant. Create step-by-step plans to achieve goals using available tools."
                        },
                        {
                            "role": "user",
                            "content": prompt
                        }
                    ],
                    "temperature": config.temperature,
                    "max_tokens": Some(config.max_context_tokens as u32),
                    "stream": false
                }))
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!("NVIDIA API request failed: {}", response.status()));
            }

            let response_json: serde_json::Value = response.json().await?;
            let content = response_json["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?
                .to_string();

            Self::parse_plan_response(&content)
        } else {
            // Fallback to local llama.cpp if available
            let llama_endpoint = std::env::var("LLAMA_CPP_ENDPOINT").unwrap_or_else(|| "http://localhost:8080/completion".to_string());
            
            let client = reqwest::Client::new();
            let response = client
                .post(&llama_endpoint)
                .json(&serde_json::json!({
                    "prompt": format!("{} {}", 
                        "You are ORION, an autonomous AI assistant. Create a step-by-step plan to achieve the goal.\n\n",
                        prompt
                    ),
                    "temperature": config.temperature,
                    "n_predict": config.max_context_tokens as i32,
                    "stop": ["\n\n", "###"],
                    "stream": false
                }))
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow::anyhow!("Llama.cpp request failed: {}", response.status()));
            }

            let response_json: serde_json::Value = response.json().await?;
            let content = response_json["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?
                .to_string();

            Self::parse_plan_response(&content)
        }
    }

    fn parse_plan_response(response: &str) -> Result<(Vec<PlanStep>, String)> {
        // Try to find JSON in response
        let start = response.find('{').unwrap_or(0);
        let end = response.rfind('}').map(|i| i + 1).unwrap_or(response.len());
        let json_str = &response[start..end];

        let parsed: serde_json::Value = serde_json::from_str(json_str)?;

        let steps = parsed["steps"].as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|s| PlanStep {
                tool: s["tool"].as_str().unwrap_or("").to_string(),
                args: s["args"].clone(),
                depends_on: s["depends_on"].as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|v| v.as_u64().map(|x| x as usize))
                    .collect(),
                description: s["description"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        let reasoning = parsed["reasoning"].as_str().unwrap_or("No reasoning provided").to_string();

        Ok((steps, reasoning))
    }

    fn fallback_plan(request: &PlanRequest) -> (Vec<PlanStep>, String) {
        // Simple fallback: single step using first available tool
        let step = if !request.available_tools.is_empty() {
            PlanStep {
                tool: request.available_tools[0].clone(),
                args: serde_json::json!({"goal": request.goal}),
                depends_on: vec![],
                description: format!("Fallback: use {} to achieve goal", request.available_tools[0]),
            }
        } else {
            PlanStep {
                tool: "none".to_string(),
                args: serde_json::json!({}),
                depends_on: vec![],
                description: "No tools available".to_string(),
            }
        };

        (vec![step], "Fallback plan - LLM unavailable".to_string())
    }

    pub async fn execute_plan(
        &self,
        plan: &PlanResponse,
        context: serde_json::Value,
    ) -> Result<Vec<ToolResult>> {
        let mut results = Vec::new();
        let mut completed: std::collections::HashSet<usize> = std::collections::HashSet::new();

        // Execute steps in dependency order
        let mut remaining: Vec<(usize, &PlanStep)> = plan.steps.iter().enumerate().collect();

        while !remaining.is_empty() {
            let mut progress = false;

            for i in (0..remaining.len()).rev() {
                let (idx, step) = remaining[i];

                if step.depends_on.iter().all(|d| completed.contains(d)) {
                    // All dependencies met, execute
                    let call = ToolCall {
                        id: format!("step_{}", idx),
                        tool: step.tool.clone(),
                        args: step.args.clone(),
                    };

                    let start = std::time::Instant::now();
                    let result = self.execute_tool(&call).await;
                    let duration = start.elapsed().as_millis() as u64;

                    let (success, result_value, error_msg) = match result {
                        Ok(v) => (true, Some(v), None),
                        Err(e) => (false, None, Some(e.to_string())),
                    };

                    let tool_result = ToolResult {
                        id: call.id,
                        success,
                        result: result_value,
                        error: error_msg,
                        duration_ms: duration,
                    };

                    results.push(tool_result);
                    completed.insert(idx);
                    remaining.remove(i);
                    progress = true;
                }
            }

            if !progress {
                // Circular dependency or missing deps
                error!("Plan execution stalled - dependency issue");
                break;
            }
        }

        Ok(results)
    }

    async fn execute_tool(&self, call: &ToolCall) -> Result<serde_json::Value> {
        // Publish tool call to bus for execution by appropriate module
        if let Some(nats) = &self.nats {
            use futures_util::stream::StreamExt;

            let subject = format!("orion.tool.{}.execute", call.tool);
            let response_subject = format!("orion.tool.{}.response.{}", call.tool, call.id);

            let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();

            // Subscribe to response
            let mut sub = nats.subscribe(response_subject.clone()).await?;
            tokio::spawn(async move {
                if let Some(msg) = sub.next().await {
                    let _ = tx.send(msg.payload.to_vec());
                }
            });

            // Send request
            nats.publish(subject, serde_json::to_vec(&call.args)?.into()).await?;

            // Wait for response with timeout
            match tokio::time::timeout(
                std::time::Duration::from_millis(self.config.tool_timeout_ms),
                rx
            ).await {
                Ok(Ok(payload)) => {
                    let result: serde_json::Value = serde_json::from_slice(&payload)?;
                    Ok(result)
                }
                Ok(Err(_)) => Err(anyhow::anyhow!("Response channel closed")),
                Err(_) => Err(anyhow::anyhow!("Tool execution timeout")),
            }
        } else {
            Err(anyhow::anyhow!("NATS not connected"))
        }
    }
}