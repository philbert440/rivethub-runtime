#!/usr/bin/env python3
"""The mechanical gate for the RivetHub runtime.

usage:
  gate.py [<repo-root>]        full gate: crate map + dependency direction, forbidden patterns, file ceiling,
                               crate manifests, privacy scan, secret scan over tracked files
  gate.py --staged             pre-commit: forbidden patterns on staged .rs files; privacy + secret scan on staged added lines
  gate.py --commit-msg <file>  commit-msg hook: house identity, no AI attribution trailers
  gate.py --commits <range>    CI: the same check over every commit in a git range
  gate.py --text               privacy + secret scan of stdin (added lines of a diff, a PR body)

Design source: docs/ARCHITECTURE.md (layers), docs/BUILDER-RULES.md (rules), docs/CONTRACT.md (what must match).
Exit 1 on any blocking finding."""
import hashlib
import json
import os
import re
import subprocess
import sys
import tomllib

LAYER = {
    'wire': 0, 'config': 0,
    'kernel': 1, 'testkit': 1,
    'memory-postgres': 2, 'memory-sqlite': 2, 'providers': 2, 'harness-acp': 2, 'terminal': 2, 'mcp': 2, 'capture': 2,
    'gateway': 3, 'agent-loop': 3, 'workers': 3, 'tasks': 3,
    'cli': 4, 'rivethub': 4,
}
ADAPTER, SERVICE, COMPOSITION = 2, 3, 4
BINARY_CRATE = 'rivethub'
FILE_CEILING = 600

PROD = [
    (re.compile(r'^\s*//|/\*'), 'COMMENT', 'comment'),
    (re.compile(r'#!?\[(allow|expect)\('), 'ALLOW', 'allow/expect attribute'),
    (re.compile(r'\.(unwrap|expect)\('), 'UNWRAP', 'unwrap/expect in production code'),
    (re.compile(r'\bunsafe\b'), 'UNSAFE', 'unsafe'),
    (re.compile(r'\bJs(Value|String|Object|Number|Array)\b|\bserde_json::Number\b|use serde_json::\{?[^;]*\bNumber\b|\w*truthy\w*|\btypeof\w*|\[object Object\]|\b\w*(NumberOr|OrNumber)\w*'), 'JSMODEL', 'JavaScript value model or semantics (truthiness, typeof, [object Object])'),
    (re.compile(r'^impl(<[^>]*>)?\s+(PartialEq|Eq|Clone|Copy|Debug|Default|Hash|PartialOrd|Ord)\s+for\b'), 'MANUALDERIVE', 'hand-written impl of a derivable trait (use #[derive])'),
    (re.compile(r'^impl(<\'de>)?\s+(serde::)?(Serialize|Deserialize<\'de>)\s+for\b|(^|[^_])\bwith\s*=\s*"(?!(::)?serde_with::)|serialize_with\s*=|deserialize_with\s*='), 'CUSTOMSERDE', 'hand-written serde (use derives, #[serde(try_from/into)], strum, serde_with)'),
    (re.compile(r'^\s*pub\s+(port|\w*_port)\s*:\s*(Option<)?(i64|i32|u64|u32|usize)\b|^\s*pub\s+(count|limit|offset|total|inserted|skipped|index|attempts?|moved|dropped|len|\w+_count|\w*_tokens)\s*:\s*(Option<)?(i64|i32)\b'), 'COUNTTYPE', 'signed or oversized integer for a port or count (u16 ports; u32/u64/usize counts)'),
    (re.compile(r'\bserde_json::(Value|Map)\b|\bMap<String,\s*(serde_json::)?Value>|use serde_json::\{?[^;]*\b(Value|Map)\b'), 'UNTYPED', 'raw serde_json Value/Map outside wire::json (use a typed struct, wire::JsonObject, or wire::Extra)'),
    (re.compile(r'\bserde_yaml::(Value|Mapping|Number|Sequence)\b|use serde_yaml::\{?[^;]*\b(Value|Mapping|Number|Sequence)\b'), 'UNTYPED', 'raw serde_yaml value outside config load/resolve (deserialize into typed fields; wrong types are serde_path_to_error errors)'),
    (re.compile(r'\bIgnoredAny\b'), 'UNKNOWNKEYS', 'IgnoredAny key capture (report unknown keys with serde_ignored)'),
    (re.compile(r'match\s+\w+(\.\w+\(\))*\.as_str\(\)\s*\{'), 'DISPATCH', 'string dispatch on a wire tag (use an enum)'),
    (re.compile(r'\b\w*([Dd]en_|_den\b|Den[A-Z]|[Rr]oom_|_room\b|Room[A-Z])\w*'), 'NAMING', 'den/room identifier'),
    (re.compile(r'tokio::runtime::Builder|#\[tokio::main\]'), 'RUNTIME', 'runtime construction outside the binary'),
    (re.compile(r'\bstd::sync::(Mutex|RwLock)\b'), 'STDLOCK', 'std lock (use parking_lot or tokio::sync)'),
    (re.compile(r'\b(println|eprintln|print|eprint)!\('), 'PRINT', 'print macro (use tracing; only the binary writes to stdout)'),
    (re.compile(r'^\s*(pub(\([a-z]+\))?\s+)?(const|static)\s+\w+\s*:\s*(&\[&str\]|\[&str;|&\[\(&str,|\[\(&str,)'), 'STRSET', 'table of protocol strings (use an enum with methods; unknown keys come from serde)'),
    (re.compile(r'^\s*(pub(\([a-z]+\))?\s+)?mod\s+(den|room|rooms)\b'), 'NAMING', 'den/room module name'),
    (re.compile(r'^\s*macro_rules!'), 'TYPEMACRO', 'macro_rules! in production code (write the types and impls out)'),
    (re.compile(r'^\s*(pub(\([a-z]+\))?\s+)?struct\s+\w+\s*\{\s*\}'), 'OPAQUE', 'empty struct that swallows a section (type its fields, or hold free-form data in the sanctioned holder)'),
    (re.compile(r'forward_to_deserialize_any!|impl<\'de>\s+(serde::)?(de::)?Deserializer<\'de>\s+for|^\s*(Null|Bool\(bool\)|Seq\(Vec<)'), 'DYNVALUE', 'home-made dynamic value model (deserialize into typed structs)'),
]
ALWAYS = {'COMMENT', 'UNSAFE'}

UUID_RE = re.compile(r'\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b', re.I)
EMAIL_RE = re.compile(r'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b')
HOME_RE = re.compile(r'/home/([A-Za-z0-9._-]+)')
IPV4_RE = re.compile(r'\b(\d{1,3}(?:\.\d{1,3}){1,3})\b')
IPV4_FULL_RE = re.compile(r'\b(?:\d{1,3}\.){3}\d{1,3}(?:/\d{1,2})?\b')
IPV6_INTERNAL_RE = re.compile(r'\b(?:f[cd][0-9a-f]{2}|fe80)(?::[0-9a-f]{0,4}){2,}\b', re.I)
WG_KEY_RE = re.compile(r'\b[A-Za-z0-9+/]{43}=(?![A-Za-z0-9+/=])')
LAN_HOST_RE = re.compile(r'\b[a-z0-9][a-z0-9-]*\.lan\b(?!\.)', re.I)
WORD_RE = re.compile(r'\b[A-Za-z][A-Za-z0-9]*\b')
NONSPACE_RE = re.compile(r'\S+')
HYPHEN_RE = re.compile(r'\b[A-Za-z][A-Za-z0-9]*(?:-[A-Za-z0-9]+)+\b')
TOKEN_RES = [
    (re.compile(r'\bsk-[A-Za-z0-9_-]{20,}'), 'API key shape (sk-…)'),
    (re.compile(r'\bgh[pousr]_[A-Za-z0-9]{30,}'), 'GitHub token'),
    (re.compile(r'\bxox[abprs]-[A-Za-z0-9-]{10,}'), 'Slack token'),
    (re.compile(r'\bAKIA[0-9A-Z]{16}\b'), 'AWS access key'),
    (re.compile(r'-----BEGIN [A-Z ]*PRIVATE KEY-----'), 'private key block'),
    (re.compile(r'\b[0-9]{17,19}:[A-Za-z0-9_-]{35}\b'), 'Telegram bot token'),
    (re.compile(r'\bMT[A-Za-z0-9]{22,}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,}'), 'Discord bot token'),
]
ALLOWED_HOME_USERS = {'ubuntu', 'runner', 'rivet', 'rivetos', 'rivethub', 'user', 'example'}
SKIP_SCAN_RE = [
    re.compile(r'^scripts/(privacy|secret)-denylist\.json$'),
    re.compile(r'^scripts/gate\.py$'),
    re.compile(r'(^|/)Cargo\.lock$'),
    re.compile(r'\.(png|jpg|jpeg|gif|webp|ico|svg|pdf|zip|gz|woff2?|ttf|otf|mp3|wav|ogg|mp4|webm|so|a|bin|wasm)$', re.I),
]
OWNER_IDENTITY_RE = [re.compile(r'(^|/)LICENSE$'), re.compile(r'(^|/)NOTICE$'), re.compile(r'^\.github/CODEOWNERS$')]
FIXTURE_RE = re.compile(r'(^|/)(fixtures|testdata|golden)/')
PRAGMA = re.compile(r'(secret-scan-allow|gate-allow)')
TITLE_RE = re.compile(r'[A-Z][a-z]+')
LINUX_AFTER_RE = re.compile(r'\s+Linux\b')
IDENT_CHAR = re.compile(r'[A-Za-z0-9-]')
PATH_MARKER_RE = [re.compile(r'(^|/)README(\.[A-Za-z0-9]+)?$', re.I), re.compile(r'(^|/)docs/'), re.compile(r'(^|/)fixtures/'), re.compile(r'\.(md|mdx)$', re.I)]
AUTHORSHIP = {
    'house': [('rivet philbot', 'rivetphilbot@gmail.com'), ('rivet', 'rivetphilbot@gmail.com'), ('philip', 'philbert440@gmail.com'),
              ('philip', 'philbert440@users.noreply.github.com'), ('philbert440', 'philbert440@gmail.com'),
              ('philbert440', 'philbert440@users.noreply.github.com')],
    'blocked_trailers': re.compile(r'^(Co-authored-by|Claude-Session|Signed-off-by):', re.I | re.M),
    'blocked_text': re.compile(r'(claude|anthropic|cursor|copilot|chatgpt|openai|generated with)', re.I),
}

findings = []


def block(msg):
    findings.append(msg)


def sha(s):
    return hashlib.sha256(s.encode()).hexdigest()


def load_hashes(root, name):
    p = os.path.join(root, 'scripts', name)
    if not os.path.exists(p):
        block(f'DENYLIST missing: scripts/{name}')
        return set()
    data = json.load(open(p))
    return {h.lower() for h in data.get('sha256', []) if re.fullmatch(r'[0-9a-f]{64}', str(h).lower())}


def is_private_ip(tok):
    parts = [int(x) for x in tok.split('/')[0].split('.')]
    if any(p > 255 for p in parts):
        return None
    a, b = parts[0], parts[1]
    if a == 10 or (a == 172 and 16 <= b <= 31) or (a == 192 and b == 168) or (a == 100 and 64 <= b <= 127):
        return 'private'
    if a in (0, 127) or (a == 169 and b == 254) or a >= 224 or (a == 192 and b == 0) or (a == 198 and b in (18, 19, 51)) or (a == 203 and b == 0):
        return None
    return 'public'


def in_tag_context(line, idx, token):
    end = idx + len(token)
    i = idx
    while i > 0 and IDENT_CHAR.match(line, i - 1):
        i -= 1
    if line[i:i + 2] == '--':
        return False
    hyphen_before = idx >= 2 and line[idx - 1] == '-' and line[idx - 2].isalnum()
    hyphen_after = end + 1 < len(line) and line[end] == '-' and line[end + 1].isalnum()
    if hyphen_before or hyphen_after:
        j = end
        while j < len(line) and IDENT_CHAR.match(line, j):
            j += 1
        return re.search(r'(^|-)(rivet|grokbot)(-|$)', line[i:j], re.I) is not None
    q = line[idx - 1] if idx > 0 else ''
    if q in ("'", '"', '`') and line[end:end + 1] == q:
        if token.lower() == 'arch':
            return False
        c = line[idx - 2] if idx >= 2 else ''
        if c.isalnum() or c in '_[(,':
            return False
        k = idx - 2
        while k >= 0 and line[k] in ' \t':
            k -= 1
        return not (k >= 0 and line[k] == ',')
    return False


def check_persona(token, line, idx):
    if len(token) <= 4:
        if TITLE_RE.fullmatch(token):
            return not LINUX_AFTER_RE.match(line, idx + len(token))
        return in_tag_context(line, idx, token)
    return True


def scan_line(line, file, priv, sec):
    out = []
    marker = any(r.search(file) for r in PATH_MARKER_RE)
    owner_file = any(r.search(file) for r in OWNER_IDENTITY_RE)
    fixture = FIXTURE_RE.search(file) is not None
    pragma = PRAGMA.search(line) is not None
    for m in UUID_RE.finditer(line):
        if sha(m.group(0).lower()) in priv:
            out.append(('denylist-uuid', m.group(0)))
    for m in EMAIL_RE.finditer(line):
        if not owner_file and sha(m.group(0).lower()) in priv:
            out.append(('denylist-email', m.group(0)))
    for m in HYPHEN_RE.finditer(line):
        tok = m.group(0)
        if UUID_RE.fullmatch(tok):
            continue
        i = m.start()
        while i > 0 and re.match(r'[A-Za-z0-9-]', line[i - 1]):
            i -= 1
        if line[i:i + 2] == '--':
            continue
        parts = tok.lower().split('-')
        agent_tag = 'rivet' in parts or 'grokbot' in parts
        if sha(tok.lower()) in priv or (agent_tag and any(p and sha(p) in priv for p in parts)):
            out.append(('denylist-tag', tok))
    for m in WORD_RE.finditer(line):
        tok = m.group(0)
        if check_persona(tok, line, m.start()):
            if not owner_file and sha(tok.lower()) in priv:
                out.append(('denylist-persona', tok))
        elif re.fullmatch(r'[A-Za-z]{2,}\d+', tok) and sha(tok.lower()) in priv:
            out.append(('denylist-host', tok))
    for m in IPV4_RE.finditer(line):
        tok = m.group(1)
        parts = tok.split('.')
        cands = [tok] + ['.'.join(parts[:n]) for n in (3, 2) if len(parts) >= n]
        if any(sha(c) in priv for c in cands):
            out.append(('denylist-ip-prefix', tok))
    for m in IPV4_FULL_RE.finditer(line):
        if not pragma and is_private_ip(m.group(0)) == 'private':
            out.append(('private-ip', m.group(0)))
    for m in HOME_RE.finditer(line):
        user = m.group(1)
        if user in ALLOWED_HOME_USERS:
            continue
        if sha(user.lower()) in priv or marker:
            out.append(('home-path', m.group(0)))
    if not pragma:
        for m in IPV6_INTERNAL_RE.finditer(line):
            out.append(('ipv6-internal', m.group(0)))
        for m in WG_KEY_RE.finditer(line):
            out.append(('wg-key', m.group(0)[:12] + '…'))
        for m in LAN_HOST_RE.finditer(line):
            out.append(('lan-host', m.group(0)))
        if not fixture:
            for rx, label in TOKEN_RES:
                if rx.search(line):
                    out.append(('secret', label))
    for tok in NONSPACE_RE.findall(line):
        if len(tok) >= 8 and (sha(tok) in sec or sha(tok.lower()) in sec):
            out.append(('denylist-secret', tok[:6] + '…'))
    return out


def scan_text(text, file, priv, sec, offset_lines=None):
    for n, line in enumerate(text.split('\n'), 1):
        for rule, match in scan_line(line, file, priv, sec):
            where = offset_lines[n - 1] if offset_lines else n
            block(f'PRIVACY {file}:{where}: {rule}: {match}')


def skip_scan(file):
    return any(r.search(file) for r in SKIP_SCAN_RE)


def git(root, *args):
    return subprocess.run(['git', *args], cwd=root, capture_output=True, check=False).stdout.decode('utf-8', errors='replace')


def manifests(root):
    crates = {}
    cdir = os.path.join(root, 'crates')
    for name in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
        p = os.path.join(cdir, name, 'Cargo.toml')
        if not os.path.exists(p):
            continue
        t = tomllib.load(open(p, 'rb'))
        pkg = t.get('package', {})
        if pkg.get('name') != name:
            block(f'MANIFEST {name}: package.name must equal the directory name')
        if t.get('lints', {}).get('workspace') is not True:
            block(f'MANIFEST {name}: missing [lints] workspace = true')
        for key in ('edition', 'license', 'version', 'publish'):
            v = pkg.get(key)
            if not (isinstance(v, dict) and v.get('workspace') is True):
                block(f'MANIFEST {name}: package.{key} must be workspace = true')
        for sec in ('dependencies', 'dev-dependencies', 'build-dependencies'):
            for dep, spec in t.get(sec, {}).items():
                if isinstance(spec, dict) and 'path' in spec:
                    continue
                if not (isinstance(spec, dict) and spec.get('workspace') is True):
                    block(f'MANIFEST {name}: {sec}.{dep} must come from [workspace.dependencies]')
        deps = {k for sec in ('dependencies', 'dev-dependencies') for k, v in t.get(sec, {}).items() if isinstance(v, dict) and 'path' in v}
        dev = {k for k, v in t.get('dev-dependencies', {}).items() if isinstance(v, dict) and 'path' in v}
        crates[name] = (deps, dev)
        if name not in LAYER:
            block(f'DEP {name}: not in the crate map (change docs/ARCHITECTURE.md and LAYER first)')
    return crates


def dependency_direction(crates):
    for name, (deps, dev) in crates.items():
        me = LAYER.get(name, 9)
        for d in deps:
            them = LAYER.get(d, 9)
            if d == 'testkit' and d in dev:
                continue
            sideways = (me == ADAPTER and them in (ADAPTER, SERVICE)) or (me == SERVICE and them in (ADAPTER, SERVICE))
            if them > me or sideways:
                block(f'DEP {name} -> {d}: forbidden (adapters and services see only kernel, wire, config)')


def check_rust_file(rel, text, crate):
    lines = text.split('\n')
    if len(re.findall(r'Self::\w+\s*=>\s*"', text)) >= 3:
        block(f'DUPNAMES {rel}: match table mapping variants to strings (derive strum Display/EnumString with serde\'s rename rule) → docs/PATTERNS.md#dupnames')
    if crate == 'wire' and '/tests/' not in '/' + rel.replace(os.sep, '/'):
        in_payload = False
        for i, line in enumerate(lines, 1):
            if re.match(r'\s*#\[derive\([^)]*(Serialize|Deserialize)', line):
                in_payload = True
            elif in_payload and re.match(r'\s*pub (struct|enum) ', line):
                in_payload = line.rstrip().endswith('{') and ' struct ' in line
            elif in_payload and re.match(r'^}', line):
                in_payload = False
            elif in_payload and re.match(r'^    [a-z_][a-z0-9_]*\s*:', line):
                block(f'WIREPUB {rel}:{i}: private field on a wire payload (wire payloads have pub fields, no accessors) → docs/PATTERNS.md#wirepub')
    is_prod = '/tests/' not in '/' + rel.replace(os.sep, '/') and crate != 'testkit'
    if is_prod:
        struct_name = ''
        pending_rename = None
        for i, line in enumerate(lines, 1):
            if '#[cfg(test)]' in line:
                break
            m = re.match(r'\s*(pub(\([a-z]+\))?\s+)?(struct|enum)\s+(\w+)', line)
            if m:
                struct_name = m.group(4)
            if 'Option<Option<' in line and not re.search(r'(Patch|Update)$', struct_name):
                block(f'NULLABLE {rel}:{i}: Option<Option<T>> outside a *Patch/*Update body (absent and null are the same unless null means "clear") → docs/PATTERNS.md#nullable')
            r = re.search(r'#\[serde\([^\]]*\brename\s*=\s*"(\w+)"', line)
            if r:
                pending_rename = (r.group(1), i)
                continue
            if pending_rename and re.match(r'\s*#\[', line):
                continue
            if pending_rename:
                f = re.match(r'\s*(pub(\([a-z]+\))?\s+)?(r#)?(\w+)\s*:', line)
                if f and f.group(4) == pending_rename[0]:
                    block(f'REDUNDANTRENAME {rel}:{pending_rename[1]}: serde rename equal to the field name → docs/PATTERNS.md#redundantrename')
                pending_rename = None
            if crate == 'config' and re.match(r'\s*pub\s+(\w+_ms|\w*timeout\w*|\w+_tokens|\w+_dims|\w*minutes|\w*hours|start|end|port|\w+_count|\w+_window|top_k|seed|num_ctx|\w*max_\w+|\w*min_tokens)\s*:\s*(Option<)?f(32|64)\b', line):
                block(f'COUNTTYPE {rel}:{i}: float for a duration, count or size setting (use u64/u32/u16/u8) → docs/PATTERNS.md#counttype')
            if re.match(r'\s*pub\s+pid\s*:\s*(Option<)?i(32|64)\b', line):
                block(f'COUNTTYPE {rel}:{i}: signed pid (use u32) → docs/PATTERNS.md#counttype')
    if re.search(r'(^|/)(den|room|rooms)(\.rs|/)', rel.replace(os.sep, '/')):
        block(f'NAMING {rel}: den/room file or module name → docs/PATTERNS.md#naming')
    is_test = '/tests/' in '/' + rel or crate == 'testkit' or rel.endswith('/build.rs')
    if not is_test:
        n = sum(1 for l in lines if l.strip())
        if n > FILE_CEILING:
            block(f'SIZE {rel}: {n} lines (ceiling {FILE_CEILING})')
    in_test = False
    for i, line in enumerate(lines, 1):
        if '#[cfg(test)]' in line:
            in_test = True
        for rx, code, label in PROD:
            if code == 'RUNTIME' and crate == BINARY_CRATE:
                continue
            if code == 'PRINT' and crate in (BINARY_CRATE, 'cli'):
                continue
            if code == 'NAMING' and re.search(r'"[^"]*(den|room)[^"]*"', line):
                continue
            if code == 'UNTYPED' and rel.replace(os.sep, '/').endswith('crates/wire/src/json.rs'):
                continue
            if code == 'UNTYPED' and 'serde_yaml' in line and re.search(r'crates/config/src/(load|resolve|free_form)\.rs$', rel.replace(os.sep, '/')):
                continue
            if (in_test or is_test) and code not in ALWAYS:
                continue
            if rx.search(line):
                block(f'{code} {rel}:{i}: {label} → docs/PATTERNS.md#{code.lower()}: {line.strip()[:80]}')


def pattern_registry(root):
    p = os.path.join(root, 'docs', 'PATTERNS.md')
    heads = set(re.findall(r'^## ([A-Z]+)$', open(p).read(), re.M)) if os.path.exists(p) else set()
    for _, code, _ in PROD:
        if code not in heads:
            block(f'PATTERNS rule {code} has no entry in docs/PATTERNS.md')


def snake(name):
    return re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()


def single_variant_tags(files):
    single = set()
    for _, text in files:
        for m in re.finditer(r'pub enum (\w+)\s*\{([^}]*)\}', text):
            variants = [v for v in re.findall(r'^\s*([A-Z]\w*)\s*[,(]?', m.group(2), re.M)]
            if len(variants) == 1:
                single.add(m.group(1))
    tag = re.compile(r'^\s*pub\s+r#type\s*:\s*(Option<)?(\w+)')
    for rel, text in files:
        for i, line in enumerate(text.split('\n'), 1):
            m = tag.match(line)
            if m and (m.group(2) in single or m.group(2) == 'String'):
                block(f'TAGFIELD {rel}:{i}: constant type tag `{m.group(2)}` as a field (use #[serde(tag = "type")]) → docs/PATTERNS.md#tagfield')


def duplicate_enums(files):
    seen = {}
    for rel, text in files:
        for m in re.finditer(r'pub enum (\w+)\s*\{([^}]*)\}', text):
            variants = tuple(sorted(re.findall(r'^\s*([A-Z]\w*)', m.group(2), re.M)))
            if len(variants) >= 3:
                seen.setdefault(variants, []).append(f'{rel}:{m.group(1)}')
    for variants, where in seen.items():
        groups = {}
        for w in where:
            groups.setdefault(re.findall(r'[A-Z][a-z0-9]*', w.rsplit(':', 1)[1])[-1], []).append(w)
        for same in groups.values():
            if len(same) > 1:
                block(f'DUPTYPE same enum defined {len(same)} times: {", ".join(same)} → docs/PATTERNS.md#duptype')


def lint_policy(root):
    head = git(root, 'show', 'main:Cargo.toml') or git(root, 'show', 'origin/main:Cargo.toml')
    if not head:
        return
    try:
        base = tomllib.loads(head).get('workspace', {}).get('lints')
        mine = tomllib.load(open(os.path.join(root, 'Cargo.toml'), 'rb')).get('workspace', {}).get('lints')
    except tomllib.TOMLDecodeError:
        return
    if base != mine:
        block('LINTPOLICY [workspace.lints] differs from main (lint policy changes only in a reviewed main commit) → docs/PATTERNS.md#lintpolicy')


def stringly(root, files):
    single_variant_tags(files)
    duplicate_enums(files)
    enums = set()
    for _, text in files:
        enums.update(re.findall(r'^\s*pub enum (\w+)', text, re.M))
    names = {}
    for e in enums:
        if not re.search(r'(Id|Kind|Status|Role|Phase|Mode|State|Source|Activity)$', e):
            continue
        names[snake(e)] = e
        if e.endswith('Id') and len(e) > 2:
            names[snake(e[:-2])] = e
    field = re.compile(r'^\s*pub\s+(\w+)\s*:\s*(Option<)?(Vec<)?String\b')
    for rel, text in files:
        for i, line in enumerate(text.split('\n'), 1):
            m = field.match(line)
            if m and m.group(1) in names:
                block(f'STRINGLY {rel}:{i}: field `{m.group(1)}` is a String but enum `{names[m.group(1)]}` exists → docs/PATTERNS.md#stringly')


def full(root):
    pattern_registry(root)
    lint_policy(root)
    crates = manifests(root)
    dependency_direction(crates)
    sources = []
    for dp, dn, fn in os.walk(os.path.join(root, 'crates')):
        dn[:] = [d for d in dn if d not in ('target', 'golden', 'fixtures')]
        for f in fn:
            if f.endswith('.rs'):
                path = os.path.join(dp, f)
                rel = os.path.relpath(path, root)
                text = open(path, errors='replace').read()
                check_rust_file(rel, text, rel.split(os.sep)[1])
                if '/tests/' not in '/' + rel and rel.split(os.sep)[1] != 'testkit':
                    sources.append((rel, text))
    stringly(root, sources)
    priv, sec = load_hashes(root, 'privacy-denylist.json'), load_hashes(root, 'secret-denylist.json')
    for rel in git(root, 'ls-files').split('\n'):
        if not rel or skip_scan(rel):
            continue
        try:
            text = open(os.path.join(root, rel), errors='strict').read()
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        scan_text(text, rel, priv, sec)
    return len(crates)


def staged(root):
    priv, sec = load_hashes(root, 'privacy-denylist.json'), load_hashes(root, 'secret-denylist.json')
    files = [f for f in git(root, 'diff', '--cached', '--name-only', '--diff-filter=ACMR').split('\n') if f]
    for rel in files:
        if skip_scan(rel):
            continue
        text = git(root, 'show', f':{rel}')
        if '\ufffd' in text[:4096]:
            continue
        if rel.startswith('crates/') and rel.endswith('.rs'):
            check_rust_file(rel, text, rel.split('/')[1])
        diff = git(root, 'diff', '--cached', '-U0', '--', rel)
        added, numbers, n = [], [], 0
        for line in diff.split('\n'):
            m = re.match(r'^@@ -\d+(?:,\d+)? \+(\d+)', line)
            if m:
                n = int(m.group(1))
                continue
            if line.startswith('+') and not line.startswith('+++'):
                added.append(line[1:])
                numbers.append(n)
                n += 1
            elif not line.startswith('-') and not line.startswith('\\'):
                n += 1
        if added:
            scan_text('\n'.join(added), rel, priv, sec, numbers)
    return len(files)


def identity_ok(name, email):
    return (name.lower().strip(), email.lower().strip()) in AUTHORSHIP['house']


def commit_message(text, author=None):
    if AUTHORSHIP['blocked_trailers'].search(text):
        block('AUTHORSHIP commit message carries a Co-authored-by / Claude-Session / Signed-off-by trailer')
    if AUTHORSHIP['blocked_text'].search(text) and re.search(r'(generated with|session_|co-authored)', text, re.I):
        block('AUTHORSHIP commit message carries AI attribution text')
    if author and not identity_ok(*author):
        block(f'AUTHORSHIP {author[0]} <{author[1]}> is not a house identity')


def commits(root, rng):
    out = git(root, 'log', '--format=%H%x1f%an%x1f%ae%x1f%B%x1e', rng)
    for rec in out.split('\x1e'):
        rec = rec.strip('\n')
        if not rec:
            continue
        h, an, ae, body = rec.split('\x1f', 3)
        before = len(findings)
        commit_message(body, (an, ae))
        for i in range(before, len(findings)):
            findings[i] = f'{findings[i]} ({h[:8]})'


def main():
    args = sys.argv[1:]
    if args[:1] == ['--commit-msg']:
        name = git('.', 'config', 'user.name').strip()
        email = git('.', 'config', 'user.email').strip()
        commit_message(open(args[1]).read(), (name, email))
        label = 'commit-msg'
    elif args[:1] == ['--staged']:
        label = f'staged ({staged(os.getcwd())} files)'
    elif args[:1] == ['--text']:
        root = os.getcwd()
        priv, sec = load_hashes(root, 'privacy-denylist.json'), load_hashes(root, 'secret-denylist.json')
        scan_text(sys.stdin.read(), '<input>', priv, sec)
        label = 'text'
    elif args[:1] == ['--commits']:
        commits(os.getcwd(), args[1])
        label = f'commits {args[1]}'
    else:
        root = os.path.abspath(args[0]) if args else os.getcwd()
        label = f'{full(root)} crates'
    for f in findings:
        print(f)
    print(f'GATE {"FAIL" if findings else "OK"}: {len(findings)} finding(s), {label}')
    sys.exit(1 if findings else 0)


if __name__ == '__main__':
    main()
