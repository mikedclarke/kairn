#!/usr/bin/env python3
"""Help-text eval: can a small model drive the kairn CLI from --help alone?

The CLI's quality bar is that any model, including small local ones, can
operate it with no guidance beyond its own help text. This harness holds
the CLI to that: per scenario it builds a scratch vault, hands the model
`kairn --help` and a task, executes the commands the model asks for, and
then checks the vault (or the model's answer) deterministically.

Needs an OpenAI-compatible chat endpoint (llama.cpp, llama-swap, Ollama,
or a hosted API) and a built `kairn` binary. Stdlib only.

  scripts/help-text-eval.py --model qwen3.5-9b
  scripts/help-text-eval.py --model gemma-4-e4b --scenario done-ambiguous -v

Exit 0 when every scenario passes, 1 otherwise.
"""

import argparse
import datetime as dt
import json
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

SYSTEM_PROMPT = """\
You are operating a command-line tool called `kairn` to complete one task.
The only documentation you have is the tool's own --help output, shown in
the first message. You may also run `kairn <command> --help` to read any
subcommand's help.

Rules:
- Reply with exactly ONE line per turn: a single command starting with
  `kairn`. No explanations, no code fences, no shell operators.
- After each command you will be shown its exit code, stdout, and stderr.
- When the task is complete, reply with the single word: DONE
- If the task asks a question, reply instead with: ANSWER: <your answer>
- If you are certain the task cannot be completed, reply: FAIL
"""

MAX_OUTPUT_CHARS = 4000


def today() -> dt.date:
    return dt.date.today()


def daily_name(date: dt.date) -> str:
    return date.strftime("%Y%m%d") + ".md"


def build_vault(root: Path) -> None:
    """A small generic vault: a few daily notes and project notes."""
    cal = root / "Calendar"
    notes = root / "Notes"
    cal.mkdir(parents=True)
    notes.mkdir(parents=True)
    yesterday = today() - dt.timedelta(days=1)
    (cal / daily_name(today())).write_text(
        "## Tasks\n"
        "* water the plants\n"
        "* email Sam about the invoice\n"
        "* email Sam about the party\n"
        "\n"
        "## Notes\n"
        "Reviewed [[Project Phoenix]] scope this morning.\n"
    )
    (cal / daily_name(yesterday)).write_text(
        "## Tasks\n"
        "* renew the domain\n"
        "* file the expense report\n"
        "* [x] send the weekly update @done(%s)\n" % yesterday.isoformat()
    )
    (notes / "Project Phoenix.md").write_text(
        "# Project Phoenix\n\n"
        "Rebuild of the reporting pipeline. Kickoff was last month.\n"
        "* draft the migration plan >%s\n" % today().isoformat()
    )
    (notes / "Home Maintenance.md").write_text(
        "# Home Maintenance\n\n"
        "The boiler service is booked for the autumn.\n"
        "Gutter clearing still needs a date.\n"
    )
    (notes / "Meeting Notes.md").write_text(
        "# Meeting Notes\n\n"
        "Agreed next steps for [[Project Phoenix]] with the team.\n"
        "Quarterly review scheduled; agenda to follow.\n"
    )


def read(root: Path, rel: str) -> str:
    path = root / rel
    return path.read_text() if path.exists() else ""


def today_note(root: Path) -> str:
    return read(root, "Calendar/" + daily_name(today()))


class Scenario:
    def __init__(self, name, task, check, answer_check=None):
        self.name = name
        self.task = task
        self.check = check          # (vault_root) -> bool, state on disk
        self.answer_check = answer_check  # (answer_text) -> bool, for ANSWER: tasks


SCENARIOS = [
    Scenario(
        "add-today",
        "Add a task to today's note: email the accountant about the VAT return",
        lambda r: "* email the accountant about the VAT return" in today_note(r),
    ),
    Scenario(
        "add-date",
        "Add a task 'book dentist appointment' to the daily note for "
        + (today() + dt.timedelta(days=4)).isoformat(),
        lambda r: "* book dentist appointment"
        in read(r, "Calendar/" + daily_name(today() + dt.timedelta(days=4))),
    ),
    Scenario(
        "capture",
        "Quickly capture this thought into today's note: "
        "idea: dark mode for the editor",
        lambda r: "idea: dark mode for the editor" in today_note(r),
    ),
    Scenario(
        "done-simple",
        "Mark the task about watering the plants as done.",
        lambda r: "[x] water the plants" in today_note(r),
    ),
    # Two tasks contain "email Sam"; a first try that matches both gets the
    # ambiguity exit and must refine. A needle specific enough to one-shot
    # it also passes: either way the right task, and only it, ends done.
    Scenario(
        "done-ambiguous",
        "Mark the task about emailing Sam regarding the invoice as done.",
        lambda r: "[x] email Sam about the invoice" in today_note(r)
        and "* email Sam about the party" in today_note(r),
    ),
    Scenario(
        "search",
        "Which note mentions the boiler service? Give the note's title.",
        lambda r: True,
        answer_check=lambda a: "home maintenance" in a.lower(),
    ),
    Scenario(
        "backlinks",
        "Which notes contain links to the note called Project Phoenix? "
        "Give their titles.",
        lambda r: True,
        answer_check=lambda a: "meeting notes" in a.lower(),
    ),
    Scenario(
        "overdue",
        "How many open tasks are overdue (due before today)? Reply with the number.",
        lambda r: True,
        answer_check=lambda a: re.search(r"\b2\b", a),
    ),
]


def chat(base_url, model, messages, timeout):
    # Sampling is left to the endpoint's per-model defaults: thinking models
    # loop under forced greedy decoding, and tuned samplers are part of what
    # is being tested. max_tokens covers reasoning tokens too.
    body = json.dumps(
        {"model": model, "messages": messages, "max_tokens": 2000}
    ).encode()
    req = urllib.request.Request(
        base_url.rstrip("/") + "/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.load(resp)
    return data["choices"][0]["message"]["content"] or ""


def extract_reply(text: str):
    """The command, DONE, ANSWER, or FAIL — leniently pulled from the reply.

    Small models wrap output in fences or prefix a stray word despite the
    protocol; the gate is help-text quality, not formatting obedience, so
    take the first line that parses rather than failing the turn.
    """
    text = re.sub(r"```[a-z]*", "", text).replace("```", "")
    for line in text.splitlines():
        line = line.strip().strip("`").rstrip(";")
        if not line:
            continue
        if line.upper() == "DONE":
            return ("done", None)
        if line.upper() == "FAIL":
            return ("fail", None)
        if line.upper().startswith("ANSWER:"):
            return ("answer", line[7:].strip())
        if line.startswith("$ "):
            line = line[2:]
        if line == "kairn" or line.startswith("kairn "):
            return ("command", line)
    return ("noise", text.strip())


def run_kairn(kairn: Path, args, vault: Path):
    proc = subprocess.run(
        [str(kairn)] + args,
        capture_output=True,
        text=True,
        timeout=60,
        env={"KAIRN_ROOT": str(vault), "PATH": "/usr/bin:/bin"},
    )
    out = proc.stdout[:MAX_OUTPUT_CHARS]
    err = proc.stderr[:MAX_OUTPUT_CHARS]
    return proc.returncode, out, err


def run_scenario(scn, opts, help_text):
    vault = Path(tempfile.mkdtemp(prefix="kairn-eval-"))
    try:
        build_vault(vault)
        messages = [
            {"role": "system", "content": SYSTEM_PROMPT},
            {
                "role": "user",
                "content": "Output of `kairn --help`:\n\n"
                + help_text
                + "\n\nTask: "
                + scn.task,
            },
        ]
        transcript = []
        answer = None
        outcome = "max-turns"
        for _ in range(opts.max_turns):
            reply = chat(opts.base_url, opts.model, messages, opts.timeout)
            kind, value = extract_reply(reply)
            transcript.append((">", reply.strip()))
            if kind == "done":
                if scn.answer_check:
                    # The task wanted an answer; one nudge, as any agent
                    # harness would give, before calling it a miss.
                    feedback = (
                        "The task asks a question; reply ANSWER: <your answer>."
                    )
                    transcript.append(("<", feedback))
                    messages.append({"role": "assistant", "content": reply})
                    messages.append({"role": "user", "content": feedback})
                    continue
                outcome = "done"
                break
            if kind == "fail":
                outcome = "gave-up"
                break
            if kind == "answer":
                answer = value
                outcome = "answered"
                break
            if kind == "noise":
                feedback = (
                    "Protocol reminder: reply with one `kairn ...` command, "
                    "DONE, ANSWER: <text>, or FAIL."
                )
            else:
                try:
                    argv = shlex.split(value)[1:]
                except ValueError as e:
                    argv, feedback = None, f"could not parse that command: {e}"
                if argv is not None:
                    code, out, err = run_kairn(opts.kairn, argv, vault)
                    feedback = f"exit code: {code}\nstdout:\n{out}\nstderr:\n{err}"
            transcript.append(("<", feedback))
            messages.append({"role": "assistant", "content": reply})
            messages.append({"role": "user", "content": feedback})
        state_ok = bool(scn.check(vault))
        if scn.answer_check:
            passed = outcome == "answered" and bool(scn.answer_check(answer or ""))
        else:
            passed = outcome == "done" and state_ok
        turns = sum(1 for d, _ in transcript if d == ">")
        return passed, outcome, turns, transcript
    finally:
        shutil.rmtree(vault, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--model", required=True, help="model id at the endpoint")
    ap.add_argument(
        "--base-url",
        default="http://127.0.0.1:8080/v1",
        help="OpenAI-compatible base URL (default: %(default)s)",
    )
    ap.add_argument(
        "--kairn",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "target/release/kairn",
        help="path to the kairn binary (default: target/release/kairn)",
    )
    ap.add_argument("--scenario", help="run only the scenario with this name")
    ap.add_argument("--max-turns", type=int, default=8)
    ap.add_argument(
        "--timeout", type=int, default=600, help="per-request seconds (model may cold-load)"
    )
    ap.add_argument("-v", "--verbose", action="store_true", help="print transcripts")
    opts = ap.parse_args()

    if not opts.kairn.is_file():
        sys.exit(f"kairn binary not found at {opts.kairn}; cargo build --release -p kairn-cli")
    help_text = subprocess.run(
        [str(opts.kairn), "--help"], capture_output=True, text=True
    ).stdout

    chosen = [s for s in SCENARIOS if not opts.scenario or s.name == opts.scenario]
    if not chosen:
        sys.exit(f"no scenario called {opts.scenario!r}")

    results = []
    for scn in chosen:
        passed, outcome, turns, transcript = run_scenario(scn, opts, help_text)
        results.append((scn.name, passed, outcome, turns))
        status = "PASS" if passed else "FAIL"
        print(f"{status}  {scn.name:16} {outcome} in {turns} turn(s)")
        if opts.verbose or not passed:
            for direction, text in transcript:
                for line in text.splitlines():
                    print(f"    {direction} {line}")
    failed = [name for name, passed, _, _ in results if not passed]
    print(f"\n{len(results) - len(failed)}/{len(results)} scenarios passed", end="")
    print(f"; failed: {', '.join(failed)}" if failed else "")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
