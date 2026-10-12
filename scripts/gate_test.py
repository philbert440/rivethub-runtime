#!/usr/bin/env python3
import os
import subprocess
import sys
import tempfile
import shutil

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CASES = {
    'COMMENT': ('// note\npub fn a() {}\n', 'pub fn a() {}\n'),
    'ALLOW': ('#[allow(dead_code)]\npub fn a() {}\n', 'pub fn a() {}\n'),
    'UNWRAP': ('pub fn a(x: Option<u8>) -> u8 { x.unwrap() }\n', 'pub fn a(x: Option<u8>) -> u8 { x.unwrap_or(0) }\n'),
    'UNSAFE': ('pub fn a() { unsafe {} }\n', 'pub fn a() {}\n'),
    'JSMODEL': ('pub struct A(serde_json::Number);\n', 'pub struct A(f64);\n'),
    'UNTYPED': ('pub struct A {\n    v: serde_json::Map<String, serde_json::Value>,\n}\n', 'pub struct A {\n    pub v: wire::JsonObject,\n}\n'),
    'TAGFIELD': ('pub enum SnapshotTag {\n    Snapshot,\n}\npub struct S {\n    pub r#type: SnapshotTag,\n}\n', '#[serde(tag = "type", rename = "snapshot")]\npub struct S {\n    pub v: i64,\n}\n'),
    'STRINGLY': ('pub enum HarnessId {\n    Grok,\n}\npub struct S {\n    pub harness: Option<String>,\n}\n', 'pub enum HarnessId {\n    Grok,\n}\npub struct S {\n    pub harness: Option<HarnessId>,\n}\n'),
    'DISPATCH': ('pub fn a(s: &String) -> u8 { match s.as_str() { _ => 1 } }\n', 'pub fn a(k: Kind) -> u8 { match k { Kind::A => 1 } }\n'),
    'NAMING': ('pub struct DenServer;\n', 'pub struct GatewayServer;\n'),
    'RUNTIME': ('pub fn a() { let _ = tokio::runtime::Builder::new_multi_thread(); }\n', 'pub async fn a() {}\n'),
    'STDLOCK': ('pub static A: std::sync::Mutex<u8> = std::sync::Mutex::new(0);\n', 'pub static A: parking_lot::Mutex<u8> = parking_lot::Mutex::new(0);\n'),
    'DYNVALUE': ('pub enum Node {\n    Null,\n    Bool(bool),\n    Seq(Vec<Node>),\n}\n', 'pub enum Kind {\n    Postgres,\n    Sqlite,\n}\n'),
    'STRSET': ('pub(crate) const KNOWN: [&str; 2] = ["vllm", "ollama"];\n', 'pub enum ProviderType {\n    Vllm,\n    Ollama,\n}\n'),
    'MANUALDERIVE': ('pub struct A(u8);\nimpl PartialEq for A {\n    fn eq(&self, o: &Self) -> bool { self.0 == o.0 }\n}\n', '#[derive(PartialEq)]\npub struct A(u8);\n'),
    'CUSTOMSERDE': ('pub struct V;\nimpl Serialize for V {\n}\n', '#[derive(Serialize)]\n#[serde(into = "u8")]\npub struct V;\n'),
    'COUNTTYPE': ('pub struct A {\n    pub port: i64,\n    pub limit: i64,\n}\n', 'pub struct A {\n    pub port: u16,\n    pub limit: u32,\n}\n'),
    'DUPNAMES': ('pub fn n(h: H) -> &\'static str { match h { Self::A => "a", Self::B => "b", Self::C => "c" } }\n', '#[derive(strum::Display)]\npub enum H {\n    A,\n}\n'),
    'DUPTYPE': ('pub enum Role {\n    System,\n    User,\n    Tool,\n}\npub enum CaptureRole {\n    System,\n    User,\n    Tool,\n}\n', 'pub enum Role {\n    System,\n    User,\n    Tool,\n}\n'),
    'UNKNOWNKEYS': ('pub struct A {\n    pub unknown: std::collections::BTreeMap<String, serde::de::IgnoredAny>,\n}\n', 'pub struct A {\n    pub port: u16,\n}\n'),
    'TYPEMACRO': ('macro_rules! closed {\n    ($n:ident) => { pub enum $n {} };\n}\n', 'pub enum Effort {\n    Low,\n}\n'),
    'OPAQUE': ('#[derive(Deserialize)]\npub struct OpaqueSection {}\n', '#[derive(Deserialize)]\npub struct Safety {\n    pub enabled: bool,\n}\n'),
    'NULLABLE': ('pub struct Hit {\n    pub title: Option<Option<String>>,\n}\n', 'pub struct TagPatch {\n    pub title: Option<Option<String>>,\n}\n'),
    'REDUNDANTRENAME': ('pub struct A {\n    #[serde(rename = "max_tokens")]\n    pub max_tokens: u32,\n}\n', 'pub struct A {\n    #[serde(rename = "agentId")]\n    pub agent_id: u32,\n}\n'),
    'PRINT': ('pub fn a() { println!("x"); }\n', 'pub fn a() { tracing::info!("x"); }\n'),
}
MANIFEST = '[package]\nname = "mcp"\nedition.workspace = true\nlicense.workspace = true\nversion.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n'


def gate(src, crate='mcp'):
    with tempfile.TemporaryDirectory() as d:
        shutil.copytree(os.path.join(ROOT, 'docs'), os.path.join(d, 'docs'))
        shutil.copytree(os.path.join(ROOT, 'scripts'), os.path.join(d, 'scripts'))
        os.makedirs(os.path.join(d, 'crates', crate, 'src'))
        open(os.path.join(d, 'crates', crate, 'Cargo.toml'), 'w').write(MANIFEST.replace('"mcp"', f'"{crate}"'))
        open(os.path.join(d, 'crates', crate, 'src', 'lib.rs'), 'w').write(src)
        subprocess.run(['git', 'init', '-q', d], check=True)
        out = subprocess.run([sys.executable, os.path.join(d, 'scripts', 'gate.py'), d], capture_output=True, text=True).stdout
        return {line.split()[0] for line in out.splitlines() if line and not line.startswith('GATE')}


failures = []
WIRE = {'WIREPUB': ('#[derive(Serialize)]\npub struct A {\n    hidden: u8,\n}\n', '#[derive(Serialize)]\npub struct A {\n    pub shown: u8,\n}\n')}
for code, (bad, good) in WIRE.items():
    if code not in gate(bad, 'wire'):
        failures.append(f'{code}: did not fire on the anti-pattern')
    if code in gate(good, 'wire'):
        failures.append(f'{code}: fired on the proper pattern')
for code, (bad, good) in CASES.items():
    if code not in gate(bad):
        failures.append(f'{code}: did not fire on the anti-pattern')
    if code in gate(good):
        failures.append(f'{code}: fired on the proper pattern')
EXTRA = [
    ('JSMODEL', 'pub fn is_truthy(v: &str) -> bool { !v.is_empty() }\n', 'pub fn is_enabled(v: bool) -> bool { v }\n'),
    ('ALLOW', '#[expect(dead_code)]\npub fn a() {}\n', '#[expect(dead_code, reason = "kept for the next wave")]\npub fn a() {}\n'),
    ('NAMING', 'pub fn room_state() {}\n', 'pub fn golden_path() {}\npub struct NetworkGoldenPath;\n'),
    ('JSMODEL', 'pub fn build() -> bool { "JSON.stringify(x)".is_empty() }\n', 'pub fn build() -> bool { true }\n'),
    ('COUNTTYPE', 'pub struct A {\n    pub timeout_ms: Option<f64>,\n}\n', 'pub struct A {\n    pub timeout_ms: Option<u64>,\n}\n'),
    ('COUNTTYPE', 'pub struct A {\n    pub top_k: Option<f64>,\n}\n', 'pub struct A {\n    pub top_k: Option<i32>,\n    pub top_p: Option<f64>,\n}\n'),
    ('JSMODEL', 'pub enum NumberOrText {\n    N(f64),\n}\n', 'pub struct A {\n    pub timeout_ms: u64,\n}\n'),
    ('UNTYPED', 'pub struct A {\n    pub temperature: serde_yaml::Value,\n}\n', 'pub struct A {\n    pub temperature: Option<f64>,\n}\n'),
]
for code, bad, good in EXTRA:
    if code not in gate(bad, 'config'):
        failures.append(f'{code}: did not fire on the anti-pattern (extra case)')
    if code in gate(good, 'config'):
        failures.append(f'{code}: fired on the proper pattern (extra case)')
with tempfile.TemporaryDirectory() as d:
    shutil.copytree(os.path.join(ROOT, 'docs'), os.path.join(d, 'docs'))
    shutil.copytree(os.path.join(ROOT, 'scripts'), os.path.join(d, 'scripts'))
    os.makedirs(os.path.join(d, 'crates', 'config', 'src'))
    open(os.path.join(d, 'crates', 'config', 'Cargo.toml'), 'w').write(MANIFEST.replace('"mcp"', '"config"'))
    open(os.path.join(d, 'crates', 'config', 'src', 'lib.rs'), 'w').write('mod resolve;\n')
    open(os.path.join(d, 'crates', 'config', 'src', 'resolve.rs'), 'w').write('pub fn parse(t: &str) -> Result<serde_yaml::Value, serde_yaml::Error> { serde_yaml::from_str(t) }\n')
    subprocess.run(['git', 'init', '-q', d], check=True)
    out = subprocess.run([sys.executable, os.path.join(d, 'scripts', 'gate.py'), d], capture_output=True, text=True).stdout
    if 'UNTYPED' in out:
        failures.append('UNTYPED: fired on serde_yaml::Value in config resolve.rs')
with tempfile.TemporaryDirectory() as d:
    shutil.copytree(os.path.join(ROOT, 'docs'), os.path.join(d, 'docs'))
    shutil.copytree(os.path.join(ROOT, 'scripts'), os.path.join(d, 'scripts'))
    os.makedirs(os.path.join(d, 'tools'))
    open(os.path.join(d, 'tools', 'hook.ts'), 'w').write('export const a = 1\n')
    subprocess.run(['git', 'init', '-q', d], check=True)
    subprocess.run(['git', '-C', d, 'add', '-A'], check=True)
    out = subprocess.run([sys.executable, os.path.join(d, 'scripts', 'gate.py'), d], capture_output=True, text=True).stdout
    if 'STACK tools/hook.ts' not in out:
        failures.append('STACK: did not fire on a tracked .ts file')
    if 'STACK scripts/gate.py' in out:
        failures.append('STACK: fired on the gate script itself')
with tempfile.TemporaryDirectory() as d:
    shutil.copytree(os.path.join(ROOT, 'docs'), os.path.join(d, 'docs'))
    shutil.copytree(os.path.join(ROOT, 'scripts'), os.path.join(d, 'scripts'))
    for name, deps in (('wire', '[dependencies]\nconfig = { path = "../config" }\n'), ('config', '[dependencies]\nwire = { path = "../wire" }\n')):
        os.makedirs(os.path.join(d, 'crates', name, 'src'))
        open(os.path.join(d, 'crates', name, 'Cargo.toml'), 'w').write(MANIFEST.replace('"mcp"', f'"{name}"').replace('[lints]', deps + '\n[lints]'))
        open(os.path.join(d, 'crates', name, 'src', 'lib.rs'), 'w').write('pub fn a() {}\n')
    subprocess.run(['git', 'init', '-q', d], check=True)
    out = subprocess.run([sys.executable, os.path.join(d, 'scripts', 'gate.py'), d], capture_output=True, text=True).stdout
    if 'DEP wire -> config' not in out:
        failures.append('DEP: wire -> config not refused')
    if 'DEP config -> wire' in out:
        failures.append('DEP: config -> wire refused')
for f in failures:
    print(f)
print(f'GATE-TEST {"FAIL" if failures else "OK"}: {len(CASES) + len(WIRE) + len(EXTRA) + 5} cases, {len(failures)} failure(s)')
sys.exit(1 if failures else 0)
