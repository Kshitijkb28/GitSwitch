#!/usr/bin/env python3
"""Check the IPC contract between the frontend and the Rust commands.

A mismatch here compiles and type-checks cleanly but fails at runtime, which is
exactly the class of bug the unit tests and the mocked UI checks cannot see:
a command that was never registered, a name typo, or an argument whose
camelCase spelling doesn't match the Rust parameter.
"""
import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
commands_rs = open(f"{ROOT}/src-tauri/src/commands.rs").read()
# Strip line comments: a comment after an attribute would hide the fn that follows.
commands_rs = re.sub(r"//[^\n]*", "", commands_rs)
lib_rs = open(f"{ROOT}/src-tauri/src/lib.rs").read()
api_ts = open(f"{ROOT}/src/lib/api.ts").read()

INJECTED = {"app"}  # tauri injects these, the frontend never sends them


def camel(name: str) -> str:
    head, *rest = name.split("_")
    return head + "".join(w[:1].upper() + w[1:] for w in rest)


def split_params(text: str):
    """Split a parameter list on commas that are not inside <> or ()."""
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "<(":
            depth += 1
        elif ch in ">)":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur)
    return [p.strip() for p in out if p.strip()]


# --- Rust: every #[tauri::command] and its parameters -----------------------
rust = {}
for m in re.finditer(
    r"#\[tauri::command\]\s*(?:#\[[^\]]*\]\s*)*pub\s+(?:async\s+)?fn\s+(\w+)\s*\(([^)]*)\)",
    commands_rs,
):
    name, params = m.group(1), m.group(2)
    args = set()
    for p in split_params(params):
        pname = p.split(":")[0].strip()
        if pname and pname not in INJECTED:
            args.add(camel(pname))
    rust[name] = args

# --- lib.rs: what is actually registered ------------------------------------
handler = re.search(r"generate_handler!\[(.*?)\]", lib_rs, re.S).group(1)
registered = set(re.findall(r"commands::(\w+)", handler))

# --- api.ts: every invoke() call and the keys it sends -----------------------
calls = {}
for m in re.finditer(r'invoke(?:<[^>]*>)?\(\s*"(\w+)"\s*(,\s*\{(.*?)\})?\s*\)', api_ts, re.S):
    name, _, body = m.group(1), m.group(2), m.group(3)
    keys = set()
    if body:
        for part in split_params(body):
            key = part.split(":")[0].strip()
            if key:
                keys.add(key)
    calls.setdefault(name, set()).update(keys)
# Calls that pass a variable instead of a literal object: the names are checked
# by TypeScript's parameter type, so only registration can be verified here.
passthrough = set(re.findall(r'invoke(?:<[^>]*>)?\(\s*"(\w+)"\s*,\s*[A-Za-z_]\w*\s*\)', api_ts))

failures = []
checked = 0

for name, keys in sorted(calls.items()):
    checked += 1
    if name not in rust:
        failures.append(f"{name}: called from api.ts but no #[tauri::command] defines it")
        continue
    if name not in registered:
        failures.append(f"{name}: defined but NOT in generate_handler! — would fail at runtime")
        continue
    missing = rust[name] - keys
    extra = keys - rust[name]
    if missing:
        failures.append(f"{name}: Rust expects {sorted(missing)} which api.ts never sends")
    if extra:
        failures.append(f"{name}: api.ts sends {sorted(extra)} which the Rust fn has no parameter for")

for name in sorted(passthrough):
    checked += 1
    if name not in rust:
        failures.append(f"{name}: called from api.ts but no #[tauri::command] defines it")
    elif name not in registered:
        failures.append(f"{name}: defined but NOT in generate_handler!")

for name in sorted(rust):
    if name not in registered:
        failures.append(f"{name}: is a #[tauri::command] but was never registered in lib.rs")

unused = sorted(set(rust) - set(calls) - passthrough)

print(f"commands defined in Rust : {len(rust)}")
print(f"commands registered      : {len(registered)}")
print(f"commands called from UI  : {checked}")
if unused:
    print(f"defined but never called : {', '.join(unused)}")

if failures:
    print()
    for f in failures:
        print(f"  \033[31mFAIL\033[0m {f}")
    print(f"\n\033[1mRESULT\033[0m  {len(failures)} contract problem(s)")
    sys.exit(1)

print(f"\033[32mAll {checked} invoke() calls match a registered command with identical argument names.\033[0m")
