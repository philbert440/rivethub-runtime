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
    'JSMODEL': ('pub struct A(JsNumber);\n', 'pub struct A(f64);\n'),
    'UNTYPED': ('pub struct A {\n    pub v: serde_json::Value,\n}\n', 'pub struct A {\n    pub v: u8,\n}\n'),
    'DISPATCH': ('pub fn a(s: &String) -> u8 { match s.as_str() { _ => 1 } }\n', 'pub fn a(k: Kind) -> u8 { match k { Kind::A => 1 } }\n'),
    'NAMING': ('pub struct DenServer;\n', 'pub struct GatewayServer;\n'),
    'RUNTIME': ('pub fn a() { let _ = tokio::runtime::Builder::new_multi_thread(); }\n', 'pub async fn a() {}\n'),
    'STDLOCK': ('pub static A: std::sync::Mutex<u8> = std::sync::Mutex::new(0);\n', 'pub static A: parking_lot::Mutex<u8> = parking_lot::Mutex::new(0);\n'),
    'DYNVALUE': ('pub enum Node {\n    Null,\n    Bool(bool),\n    Seq(Vec<Node>),\n}\n', 'pub enum Kind {\n    Postgres,\n    Sqlite,\n}\n'),
    'PRINT': ('pub fn a() { println!("x"); }\n', 'pub fn a() { tracing::info!("x"); }\n'),
}
MANIFEST = '[package]\nname = "mcp"\nedition.workspace = true\nlicense.workspace = true\nversion.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n'


def gate(src):
    with tempfile.TemporaryDirectory() as d:
        shutil.copytree(os.path.join(ROOT, 'docs'), os.path.join(d, 'docs'))
        shutil.copytree(os.path.join(ROOT, 'scripts'), os.path.join(d, 'scripts'))
        os.makedirs(os.path.join(d, 'crates', 'mcp', 'src'))
        open(os.path.join(d, 'crates', 'mcp', 'Cargo.toml'), 'w').write(MANIFEST)
        open(os.path.join(d, 'crates', 'mcp', 'src', 'lib.rs'), 'w').write(src)
        subprocess.run(['git', 'init', '-q', d], check=True)
        out = subprocess.run([sys.executable, os.path.join(d, 'scripts', 'gate.py'), d], capture_output=True, text=True).stdout
        return {line.split()[0] for line in out.splitlines() if line and not line.startswith('GATE')}


failures = []
for code, (bad, good) in CASES.items():
    if code not in gate(bad):
        failures.append(f'{code}: did not fire on the anti-pattern')
    if code in gate(good):
        failures.append(f'{code}: fired on the proper pattern')
for f in failures:
    print(f)
print(f'GATE-TEST {"FAIL" if failures else "OK"}: {len(CASES)} rules, {len(failures)} failure(s)')
sys.exit(1 if failures else 0)
