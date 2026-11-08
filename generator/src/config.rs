//! Small, editable project configuration. Paths are relative to this file.
use crate::Core;
use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Default)]
pub struct Config {
    pub wit: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub targets: Vec<String>,
    pub cores: Vec<Core>,
    pub lock: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
}

pub fn read(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading config {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let mut c = Config::default();
    let mut seen = std::collections::HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, value) = line.split_once('=').with_context(|| format!("{}:{}: expected key = value", path.display(), i + 1))?;
        let key = key.trim(); let value = value.trim();
        if !seen.insert(key) { bail!("duplicate config key {key}"); }
        if value.is_empty() { bail!("empty config value for {key}"); }
        match key {
            "wit" => c.wit = Some(base.join(value)),
            "out" => c.out = Some(base.join(value)),
            "targets" => c.targets = value.split(',').map(|s| s.trim().to_owned()).collect(),
            "cores" => {
                for entry in value.split(',').map(str::trim).filter(|s| *s != "none") {
                    let (lang, dir) = entry.split_once('=').map(|(l,d)| (l, Some(base.join(d)))).unwrap_or((entry, None));
                    if entry.ends_with('=') { bail!("empty core directory in config"); }
                    c.cores.push(Core { lang: lang.into(), dir });
                }
            }
            "lock" => c.lock = Some(base.join(value)),
            "api-baseline" => c.baseline = Some(base.join(value)),
            _ => bail!("unknown config key {key}"),
        }
    }
    Ok(c)
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in cwd.join(path).components() {
        match part { Component::CurDir => (), Component::ParentDir => { out.pop(); }, part => out.push(part.as_os_str()) }
    }
    out
}

/// Relative paths keep a scaffolded project movable.
fn relative(path: &Path, base: &Path, cwd: &Path) -> String {
    let path = absolute(path, cwd); let base = absolute(base, cwd);
    let p: Vec<_> = path.components().collect(); let b: Vec<_> = base.components().collect();
    if p.first() != b.first() { return path.to_string_lossy().into_owned(); }
    let common = p.iter().zip(&b).take_while(|(x,y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() { out.push(".."); }
    for part in &p[common..] { out.push(part.as_os_str()); }
    if out.as_os_str().is_empty() { ".".into() } else { out.to_string_lossy().replace('\\', "/") }
}

pub fn render(c: &Config, path: &Path, cwd: &Path) -> Result<String> {
    let base = path.parent().unwrap_or(cwd);
    let rel = |p: &Path| relative(p, base, cwd);
    if c.cores.iter().filter_map(|c| c.dir.as_ref()).any(|p| p.to_string_lossy().contains(',')) { bail!("Go core directory cannot contain commas in config"); }
    let cores = c.cores.iter().map(|c| match &c.dir { Some(d) => format!("{}={}", c.lang, rel(d)), None => c.lang.clone() }).collect::<Vec<_>>().join(",");
    let mut text = format!("# Project settings for wabi-wit-gen. Edit freely; never overwritten.\n# Paths are relative to this file. CLI options override these settings.\nwit = {}\nout = {}\n", rel(c.wit.as_ref().context("config needs wit")?), rel(c.out.as_ref().context("config needs out")?));
    if !c.targets.is_empty() { text.push_str(&format!("targets = {}\n", c.targets.join(","))); }
    text.push_str(&format!("cores = {}\n", if cores.is_empty() { "none" } else { &cores }));
    if let Some(p) = &c.lock { text.push_str(&format!("lock = {}\n", rel(p))); }
    if let Some(p) = &c.baseline { text.push_str(&format!("api-baseline = {}\n", rel(p))); }
    // The format intentionally has no shell evaluation or interpolation.
    for value in [c.wit.as_ref(), c.out.as_ref(), c.lock.as_ref(), c.baseline.as_ref()].into_iter().flatten().chain(c.cores.iter().filter_map(|c| c.dir.as_ref())) {
        let value = value.to_string_lossy();
        if value.contains(['\n', '\r']) || value.trim() != value.as_ref() { bail!("config paths cannot contain newlines or leading/trailing whitespace"); }
    }
    Ok(text)
}
