"""MBPP evaluation harness for Orion - evaluate all splits."""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from .assistant import Assistant
from .config import Config, load_config
from datasets import load_dataset

def load_mbpp_all_splits() -> list[dict]:
    """Load MBPP dataset from all splits: train, test, validation, prompt."""
    dataset = load_dataset("google-research-datasets/mbpp")
    problems = []
    for split_name in ["train", "test", "validation", "prompt"]:
        split = dataset[split_name]
        for problem in split:
            problems.append({
                "task_id": problem["task_id"],
                "prompt": problem["text"],
                "code": problem["code"],  # canonical solution
                "test_list": problem["test_list"],
                "test_setup_code": problem["test_setup_code"]
            })
    return problems

def _extract_code(answer: str, entry_point: str) -> str:
    """Pull the last complete python function for entry_point out of a chat answer."""
    if not answer:
        return ""
    # Look for code blocks
    candidates = re.findall(r"```(?:python)?\s*\n(.*?)```", answer, re.S)
    blocks = candidates or [answer]
    # Try to find function definition
    for block in reversed(blocks):
        if f"def " in block:  # Any function definition
            code = block
            break
    else:
        code = blocks[-1]
    # strip leading prose lines some models emit before the code
    lines = code.splitlines()
    while lines and not lines[0].lstrip().startswith(("def ", "from ", "import ", "class ", "@", '"""', "'''", "#")):
        lines.pop(0)
    return "\n".join(lines).rstrip()

def run_mbpp_all(cfg: Config, assistant: Assistant | None = None) -> dict:
    """Run MBPP evaluation on all splits."""
    assistant = assistant or Assistant(cfg)
    problems = load_mbpp_all_splits()
    passed, results = 0, []
    
    for i, p in enumerate(problems):
        prompt = (
            f"{p['prompt']}\n\n"
            "Write a Python function to solve this problem. "
            "Return ONLY the complete Python function in one ```python code block. "
            "No explanation, no test code."
        )
        t0 = time.time()
        try:
            answer = _answer_text(assistant.client.chat(
                [{"role": "user", "content": prompt}], temperature=0.0, max_tokens=600
            ))
            code = _extract_code(answer, f"function_{p['task_id']}")  # entry_point not in MBPP, use generic
            ok = _exec_mbpp_test(code, p)
        except Exception as e:
            ok, answer, code = False, f"ERROR: {e}", ""
        passed += ok
        results.append({
            "task_id": p["task_id"], 
            "passed": ok, 
            "latency": time.time() - t0,
            "prompt": p["prompt"][:100]  # truncate for storage
        })
        if (i + 1) % 10 == 0 or (i + 1) == len(problems):
            print(f"  [{i+1}/{len(problems)}] {p['task_id']} {'PASS' if ok else 'FAIL'}")
    
    score = passed / len(problems) if problems else 0
    summary = {
        "suite": "mbpp", 
        "n": len(problems), 
        "passed": passed,
        "pass_at_1": round(score * 100, 1)
    }
    print(json.dumps(summary))
    return {"summary": summary, "results": results}

def _answer_text(msg: dict) -> str:
    """Chat message -> text. Reasoning models may fill only reasoning_content."""
    text = msg.get("content")
    if not text:
        text = msg.get("reasoning_content") or ""
    # if reasoning contains the final answer (content empty), take it as-is;
    # otherwise prefer content.
    return text

def _exec_mbpp_test(code: str, problem: dict) -> bool:
    """Run the problem's test cases against the completion in a subprocess."""
    if not code.strip():
        return False
        
    # Build test harness
    setup = problem["test_setup_code"]
    tests = "\n".join(problem["test_list"])
    harness = f"{setup}\n\n{code}\n\n{tests}"
    
    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
        f.write(harness)
        path = f.name
    try:
        proc = subprocess.run([sys.executable, path], capture_output=True,
                              text=True, timeout=10)
        success = proc.returncode == 0
        if not success:
            # Debug output
            print(f"    Test failed: {proc.stderr[:200]}")
            print(f"    Stdout: {proc.stdout[:200]}")
        return success
    except subprocess.TimeoutExpired:
        print(f"    Test timed out")
        return False
    finally:
        os.unlink(path)

def main(argv=None) -> int:
    args = sys.argv[1:] if argv is None else argv
    cfg = load_config()
    if not cfg.api_key:
        print("no API key", file=sys.stderr)
        return 2
    out_dir = Path("results/mbpp")
    out_dir.mkdir(parents=True, exist_ok=True)
    r = run_mbpp_all(cfg)
    (out_dir / "mbpp_raw_results.json").write_text(json.dumps(r, indent=2))
    # Also save just the summary
    (out_dir / "mbpp_summary.json").write_text(json.dumps(r["summary"], indent=2))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())