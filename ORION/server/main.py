from fastapi import FastAPI, HTTPException, WebSocket, WebSocketDisconnect
from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Any
import async_nats
import json
import uuid
import asyncio
from contextlib import asynccontextmanager
import structlog

logger = structlog.get_logger()

# Global NATS connection
nats_client: Optional[async_nats.Client] = None

class TTSRequest(BaseModel):
    text: str
    voice: Optional[str] = None
    speed: float = 1.0
    sample_rate: int = 16000

class ControlAction(BaseModel):
    action_type: str
    params: Dict[str, Any]

class SearchQuery(BaseModel):
    text: Optional[str] = None
    embedding: Optional[List[float]] = None
    entity_types: Optional[List[str]] = None
    filters: Dict[str, Any] = Field(default_factory=dict)
    limit: int = 10
    offset: int = 0

class PlanRequest(BaseModel):
    goal: str
    context: Dict[str, Any] = Field(default_factory=dict)
    constraints: List[str] = Field(default_factory=list)
    available_tools: List[str] = Field(default_factory=list)

class JobDefinition(BaseModel):
    id: str
    name: str
    enabled: bool = True
    schedule: Optional[str] = None
    trigger: Dict[str, Any]
    condition: Optional[str] = None
    steps: List[Dict[str, Any]]
    on_success: List[str] = Field(default_factory=list)
    on_failure: List[str] = Field(default_factory=list)

@asynccontextmanager
async def lifespan(app: FastAPI):
    global nats_client
    nats_client = await async_nats.connect("nats://localhost:4222")
    logger.info("Connected to NATS")
    yield
    await nats_client.close()
    logger.info("Disconnected from NATS")

app = FastAPI(title="JARVIS API", version="0.1.0", lifespan=lifespan)

# Health check
@app.get("/health")
async def health():
    return {"status": "healthy", "nats": nats_client is not None and not nats_client.is_closed}

# Voice endpoints
@app.post("/voice/tts")
async def tts(request: TTSRequest):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    correlation_id = str(uuid.uuid4())
    response_subject = f"jarvis.tts.response.{correlation_id}"
    
    # Subscribe to response
    future = asyncio.get_event_loop().create_future()
    async def handler(msg):
        if not future.done():
            future.set_result(msg.data)
    sub = await nats_client.subscribe(response_subject, cb=handler)
    
    # Publish request
    await nats_client.publish("jarvis.tts", json.dumps(request.dict()).encode(), reply=response_subject)
    
    try:
        response_data = await asyncio.wait_for(future, timeout=10.0)
        return json.loads(response_data)
    except asyncio.TimeoutError:
        raise HTTPException(504, "TTS timeout")
    finally:
        await sub.unsubscribe()

@app.websocket("/voice/stream")
async def voice_stream(websocket: WebSocket):
    await websocket.accept()
    if not nats_client:
        await websocket.close(code=1011)
        return
    
    sub = await nats_client.subscribe("jarvis.transcription")
    try:
        async for msg in sub.messages:
            await websocket.send_text(msg.data.decode())
    except WebSocketDisconnect:
        pass
    finally:
        await sub.unsubscribe()

# Control endpoints
@app.post("/control/action")
async def control_action(action: ControlAction):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    correlation_id = str(uuid.uuid4())
    response_subject = f"jarvis.control.result.{correlation_id}"
    
    future = asyncio.get_event_loop().create_future()
    async def handler(msg):
        if not future.done():
            future.set_result(msg.data)
    sub = await nats_client.subscribe(response_subject, cb=handler)
    
    await nats_client.publish("jarvis.control.action", json.dumps(action.dict()).encode(), reply=response_subject)
    
    try:
        response_data = await asyncio.wait_for(future, timeout=30.0)
        return json.loads(response_data)
    except asyncio.TimeoutError:
        raise HTTPException(504, "Control action timeout")
    finally:
        await sub.unsubscribe()

@app.websocket("/control/screenshot")
async def control_screenshot(websocket: WebSocket):
    await websocket.accept()
    if not nats_client:
        await websocket.close(code=1011)
        return
    
    sub = await nats_client.subscribe("jarvis.control.screenshot")
    try:
        async for msg in sub.messages:
            await websocket.send_bytes(msg.data)
    except WebSocketDisconnect:
        pass
    finally:
        await sub.unsubscribe()

# Memory endpoints
@app.post("/memory/search")
async def memory_search(query: SearchQuery):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    correlation_id = str(uuid.uuid4())
    response_subject = f"jarvis.memory.results.{correlation_id}"
    
    future = asyncio.get_event_loop().create_future()
    async def handler(msg):
        if not future.done():
            future.set_result(msg.data)
    sub = await nats_client.subscribe(response_subject, cb=handler)
    
    await nats_client.publish("jarvis.memory.search", json.dumps(query.dict()).encode(), reply=response_subject)
    
    try:
        response_data = await asyncio.wait_for(future, timeout=10.0)
        return json.loads(response_data)
    except asyncio.TimeoutError:
        raise HTTPException(504, "Memory search timeout")
    finally:
        await sub.unsubscribe()

# Planning endpoints
@app.post("/planning/plan")
async def planning_plan(request: PlanRequest):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    correlation_id = str(uuid.uuid4())
    response_subject = f"jarvis.planning.plan_response.{correlation_id}"
    
    future = asyncio.get_event_loop().create_future()
    async def handler(msg):
        if not future.done():
            future.set_result(msg.data)
    sub = await nats_client.subscribe(response_subject, cb=handler)
    
    await nats_client.publish("jarvis.planning.plan", json.dumps(request.dict()).encode(), reply=response_subject)
    
    try:
        response_data = await asyncio.wait_for(future, timeout=30.0)
        return json.loads(response_data)
    except asyncio.TimeoutError:
        raise HTTPException(504, "Planning timeout")
    finally:
        await sub.unsubscribe()

# Automation endpoints
@app.post("/automation/jobs")
async def create_job(job: JobDefinition):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    await nats_client.publish("jarvis.automation.job.create", json.dumps({"job": job.dict()}).encode())
    return {"status": "created", "job_id": job.id}

@app.delete("/automation/jobs/{job_id}")
async def delete_job(job_id: str):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    await nats_client.publish("jarvis.automation.job.delete", json.dumps({"id": job_id}).encode())
    return {"status": "deleted", "job_id": job_id}

@app.post("/automation/jobs/{job_id}/trigger")
async def trigger_job(job_id: str, args: Dict[str, Any] = {}):
    if not nats_client:
        raise HTTPException(503, "NATS not connected")
    
    await nats_client.publish("jarvis.automation.job.trigger", json.dumps({"id": job_id, "args": args}).encode())
    return {"status": "triggered", "job_id": job_id}

# Metrics endpoint for Prometheus
@app.get("/metrics")
async def metrics():
    from prometheus_client import generate_latest, CONTENT_TYPE_LATEST
    from fastapi import Response
    return Response(content=generate_latest(), media_type=CONTENT_TYPE_LATEST)

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8080)