"""Eval harness for Orion.

Two suites:
1. HumanEval coding benchmark (pass@1) — real problems, sandboxed execution.
2. Expert-rubric eval: 50 diverse prompts scored 1-5 by a judge model on
   accuracy, depth, clarity, and style.

Run:
  python -m orion.eval humaneval [--n 40] [--k 1]
  python -m orion.eval rubric [--n 50]
  python -m orion.eval all
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from .assistant import Assistant
from .config import Config, load_config

HUMANEVAL_ROWS_URL = ("https://datasets-server.huggingface.co/rows"
                      "?dataset=openai%2Fopenai_humaneval"
                      "&config=openai_humaneval&split=test"
                      "&offset={offset}&length={length}")
HUMANEVAL_CACHE = Path("~/.orion/evals/humaneval.jsonl").expanduser()

RUBRIC_PROMPTS = [
    # coding (12)
    "Write a Python function to reverse a linked list iteratively.",
    "Find the bug: def avg(nums): return sum(nums)/len(nums) if nums else 0",
    "Explain when to use Postgres vs MongoDB for a new project.",
    "Write a SQL query for the second-highest salary per department.",
    "Review this API design: GET /users/{id}/posts?sort=recent&limit=20",
    "Write a Rust function that safely reads a file to String.",
    "Explain the difference between processes and threads with a table.",
    "Refactor advice for a 3000-line God-class controller in Python.",
    "Write a git pre-commit hook that blocks commits to main.",
    "Explain Docker layered filesystems and image size optimization.",
    "Write a regex to validate ISO-8601 dates; explain each part.",
    "Design a rate limiter for a public API (algorithm + data structure).",
    # analysis (10)
    "Compare REST, GraphQL, and gRPC for a mobile backend.",
    "Analyze the trade-offs of event sourcing vs CRUD for e-commerce.",
    "Explain Bayes' theorem with a medical-test worked example.",
    "A product's DAU rose 20% but revenue fell 5%. Hypothesize 3 causes.",
    "Explain CAP theorem and how modern DBs blur the lines.",
    "Compare TCO of on-prem vs cloud GPU clusters for ML training.",
    "Explain p-values and confidence intervals to a product manager.",
    "What are the failure modes of distributed consensus (Raft/Paxos)?",
    "Estimate how many piano tuners work in Chicago; show reasoning.",
    "Explain the security implications of JWT vs server-side sessions.",
    # writing (9)
    "Write a 100-word release note for a speed-improvement update.",
    "Draft a polite decline email for a job candidate.",
    "Summarize the plot of Hamlet in exactly 3 sentences.",
    "Rewrite this to be more concise: 'Due to the fact that we were not aware of the fact that the system was down…'",
    "Write engaging alt text for a chart showing rising CO2 levels.",
    "Draft a cold-outreach email to a security conference speaker.",
    "Write a technical blog intro paragraph about vector databases.",
    "Turn these notes into a status update: 'auth done, billing blocked on contract, need design review'",
    "Write a 6-line poem about deploying code on Friday.",
    # research/knowledge (9)
    "Explain how transformer attention works, mechanically.",
    "What caused the 2008 financial crisis? 5 key causes.",
    "Summarize the current state of fusion energy research.",
    "Explain CRISPR and its main clinical applications today.",
    "What are the main arguments for and against nuclear power?",
    "Explain how HTTPS prevents man-in-the-middle attacks.",
    "Summarize the trade war dynamics between the US and China (last 5 years).",
    "Explain quantum supremacy claims and their criticisms.",
    "How do mRNA vaccines differ from traditional vaccines technically?",
    # planning/reasoning (10)
    "Plan a 2-week sprint for a 3-person team building a mobile app MVP.",
    "Design a study plan to learn Rust in 30 days at 1h/day.",
    "Create a migration plan from monolith to microservices with 5 engineers.",
    "Plan a technical due-diligence checklist before acquiring a SaaS startup.",
    "Devise a debugging strategy for intermittent 5xx errors in production.",
    "Plan a personal knowledge-management system for a researcher.",
    "Outline an incident-response plan for a data breach at a small company.",
    "Reason step by step: if all Bloops are Razzies and some Razzies are Lazzies, can we conclude any Bloop is a Lazzie?",
    "Plan a zero-downtime database migration for a live product.",
    "What's the best strategy for a chess beginner to reach 1500 Elo?",
]

JUDGE_SYSTEM = """You are an expert evaluator scoring an AI assistant's response.
Score 1-5 on each dimension:
- accuracy: factually correct, no errors or fabrications
- depth: covers what an expert would cover; appropriately detailed
- clarity: well-structured, understandable, right length for the ask
- style: direct, professional, no filler or sycophancy
Be strict: 3 = adequate, 4 = strong, 5 = exceptional expert work.
Output ONLY valid JSON: {"accuracy": N, "depth": N, "clarity": N, "style": N,
"overall": N, "one_line_verdict": "..."} where overall is your holistic 1-5."""


# ---------------------------------------------------------------------------
def load_humaneval(max_problems: int | None = None) -> list[dict]:
    """Fetch HumanEval via the HF datasets-server rows API (paginated)."""
    HUMANEVAL_CACHE.parent.mkdir(parents=True, exist_ok=True)
    if not HUMANEVAL_CACHE.exists():
        all_rows = []
        offset = 0
        while True:
            url = HUMANEVAL_ROWS_URL.format(offset=offset, length=100)
            req = urllib.request.Request(url, headers={"Accept": "application/json"})
            with urllib.request.urlopen(req, timeout=30) as r:
                data = json.load(r)
            rows = data.get("rows", [])
            if not rows:
                break
            for row in rows:
                rec = row["row"]
                # rows API returns prompt/canonical_solution/test/entry_point
                if "prompt" in rec and "test" in rec:
                    all_rows.append(rec)
            offset += len(rows)
            if max_problems and offset >= max_problems * 3:
                break
        with open(HUMANEVAL_CACHE, "w") as f:
            for rec in all_rows:
                f.write(json.dumps(rec) + "\n")
    problems = []
    with open(HUMANEVAL_CACHE) as f:
        for line in f:
            problems.append(json.loads(line))
            if max_problems and len(problems) >= max_problems:
                break
    return problems


def _answer_text(msg: dict) -> str:
    """Chat message -> text. Reasoning models may fill only reasoning_content."""
    text = msg.get("content")
    if not text:
        text = msg.get("reasoning_content") or ""
    # if reasoning contains the final answer (content empty), take it as-is;
    # otherwise prefer content.
    return text


def _extract_code(answer: str, entry_point: str) -> str:
    """Pull the last complete python function for entry_point out of a chat answer."""
    if not answer:
        return ""
    candidates = re.findall(r"```(?:python)?\s*\n(.*?)```", answer, re.S)
    blocks = candidates or [answer]
    for block in reversed(blocks):
        if f"def {entry_point}" in block:
            code = block
            break
    else:
        for block in reversed(blocks):  # fallback: any def
            if "def " in block:
                code = block
                break
        else:
            code = blocks[-1]
    # strip leading prose lines some models emit before the code
    lines = code.splitlines()
    while lines and not lines[0].lstrip().startswith(("def ", "from ", "import ", "class ", "@", '"""', "'''", "#")):
        lines.pop(0)
    return "\n".join(lines).rstrip()


def run_humaneval(cfg: Config, n: int = 40, assistant: Assistant | None = None) -> dict:
    assistant = assistant or Assistant(cfg)
    problems = load_humaneval(n)
    passed, results = 0, []
    for i, p in enumerate(problems):
        prompt = (
            f"{p['prompt']}\n\nComplete the function `{p['entry_point']}`. "
            f"Return ONLY the complete Python function in one ```python code block. "
            f"No explanation, no test code."
        )
        t0 = time.time()
        try:
            answer = _answer_text(assistant.client.chat(
                [{"role": "user", "content": prompt}], temperature=0.0, max_tokens=600
            ))
            code = _extract_code(answer, p["entry_point"])
            ok = _exec_humaneval_test(code, p)
        except Exception as e:
            ok, answer, code = False, f"ERROR: {e}", ""
        passed += ok
        results.append({"task_id": p["task_id"], "passed": ok, "latency": time.time() - t0})
        print(f"  [{i+1}/{len(problems)}] {p['task_id']} {'PASS' if ok else 'FAIL'}")
    score = passed / len(problems)
    summary = {"suite": "humaneval", "n": len(problems), "passed": passed,
               "pass_at_1": round(score * 100, 1)}
    print(json.dumps(summary))
    return {"summary": summary, "results": results}


def _exec_humaneval_test(code: str, problem: dict) -> bool:
    """Run the problem's canonical test against the completion in a subprocess."""
    harness = (
        code + "\n\n" + problem["test"] +
        f"\n\ncheck({problem['entry_point']})\nprint('HUMANEVAL_OK')"
    )
    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
        f.write(harness)
        path = f.name
    try:
        proc = subprocess.run([sys.executable, path], capture_output=True,
                              text=True, timeout=20)
        return proc.returncode == 0 and "HUMANEVAL_OK" in proc.stdout
    except subprocess.TimeoutExpired:
        return False
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
def run_rubric(cfg: Config, n: int = 50, assistant: Assistant | None = None) -> dict:
    assistant = assistant or Assistant(cfg)
    prompts = RUBRIC_PROMPTS[:n]
    scores = []
    for i, prompt_text in enumerate(prompts):
        try:
            answer = assistant.chat(prompt_text)
            judge_out = assistant.client.chat(
                [{"role": "system", "content": JUDGE_SYSTEM},
                 {"role": "user",
                  "content": f"PROMPT:\n{prompt_text}\n\nRESPONSE:\n{answer[:6000]}"}],
                temperature=0.0, max_tokens=300, thinking=False,
            ).get("content", "")
            m = re.search(r"\{.*\}", judge_out, re.S)
            verdict = json.loads(m.group(0)) if m else {"overall": 0,
                                                        "one_line_verdict": judge_out[:120]}
        except Exception as e:
            verdict = {"overall": 0, "one_line_verdict": f"eval error: {e}"}
        scores.append({"prompt": prompt_text[:60], **verdict})
        print(f"  [{i+1}/{len(prompts)}] overall={verdict.get('overall')} "
              f"| {verdict.get('one_line_verdict', '')[:70]}")
    overalls = [s.get("overall", 0) for s in scores]
    avg = sum(overalls) / len(overalls) if overalls else 0
    summary = {"suite": "rubric", "n": len(prompts), "avg_overall": round(avg, 2),
               "four_plus_ratio": round(sum(1 for o in overalls if o >= 4) / len(overalls), 3)}
    print(json.dumps(summary))
    return {"summary": summary, "scores": scores}


# ---------------------------------------------------------------------------
def main(argv=None) -> int:
    args = sys.argv[1:] if argv is None else argv
    cfg = load_config()
    if not cfg.api_key:
        print("no API key", file=sys.stderr)
        return 2
    suite = args[0] if args else "all"
    n = 40
    if "--n" in args:
        n = int(args[args.index("--n") + 1])
    out_dir = Path("~/.orion/evals").expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    if suite in ("humaneval", "all"):
        r = run_humaneval(cfg, n)
        (out_dir / "humaneval_result.json").write_text(json.dumps(r, indent=2))
    if suite in ("rubric", "all"):
        r = run_rubric(cfg, min(n, len(RUBRIC_PROMPTS)))
        (out_dir / "rubric_result.json").write_text(json.dumps(r, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
