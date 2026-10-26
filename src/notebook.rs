use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn multiline(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}
pub fn source(cell: &Value) -> String {
    multiline(&cell["source"])
}
pub fn set_source(cell: &mut Value, text: &str) {
    cell["source"] = json!(text.split_inclusive('\n').collect::<Vec<_>>());
}
pub fn new_cell(kind: &str) -> Value {
    let mut cell = json!({"id":uuid::Uuid::new_v4().simple().to_string(),"cell_type":kind,"metadata":{},"source":[]});
    if kind == "code" {
        cell["outputs"] = json!([]);
        cell["execution_count"] = Value::Null;
    }
    cell
}
pub struct Notebook {
    pub path: PathBuf,
    pub data: Value,
    pub dirty: bool,
    original: Vec<u8>,
}
impl Notebook {
    pub fn load(path: &Path) -> Result<Self> {
        let original = fs::read(path).with_context(|| format!("Reading {}", path.display()))?;
        let data: Value = serde_json::from_slice(&original)?;
        if data["nbformat"] != 4 || !data["cells"].is_array() {
            bail!("Expected an nbformat 4 notebook");
        }
        if !data["metadata"].is_object() {
            bail!("Invalid notebook metadata");
        }
        for cell in data["cells"].as_array().unwrap() {
            let valid_source = cell["source"].is_string()
                || cell["source"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string));
            if !valid_source || !cell["metadata"].is_object() {
                bail!("Invalid cell source or metadata");
            }
            if cell["cell_type"] == "code"
                && (!cell["outputs"].is_array()
                    || !(cell["execution_count"].is_null() || cell["execution_count"].is_u64()))
            {
                bail!("Invalid cell outputs or execution count");
            }
            if !matches!(
                cell["cell_type"].as_str(),
                Some("code" | "markdown" | "raw")
            ) {
                bail!("Unknown cell type");
            }
        }
        Ok(Self {
            path: path.to_owned(),
            data,
            dirty: false,
            original,
        })
    }
    pub fn cells(&self) -> &Vec<Value> {
        self.data["cells"].as_array().unwrap()
    }
    pub fn cells_mut(&mut self) -> &mut Vec<Value> {
        self.dirty = true;
        self.data["cells"].as_array_mut().unwrap()
    }
    pub fn save(&mut self) -> Result<()> {
        if fs::read(&self.path)? != self.original {
            bail!("File changed on disk: save refused. Reload with Ctrl+L.");
        }
        let mut bytes = serde_json::to_vec_pretty(&self.data)?;
        bytes.push(b'\n');
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.as_file()
            .set_permissions(fs::metadata(&self.path)?.permissions())?;
        tmp.write_all(&bytes)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        self.original = bytes;
        self.dirty = false;
        Ok(())
    }
}
pub fn discover(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir()
                && !entry.file_name().to_string_lossy().starts_with('.')
                && !matches!(
                    entry.file_name().to_str(),
                    Some("target" | "node_modules" | "__pycache__")
                )
            {
                walk(&path, paths, depth + 1);
            } else if kind.is_file() && path.extension().is_some_and(|s| s == "ipynb") {
                paths.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(root, &mut paths, 0);
    paths.sort();
    paths
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_conflict() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("test.ipynb");
        let v = json!({"nbformat":4,"nbformat_minor":5,"metadata":{"custom":[1,2]},"cells":[{"cell_type":"markdown","metadata":{},"source":["café\n","next"],"attachments":{"x":{}}}]});
        fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        let mut n = Notebook::load(&p).unwrap();
        assert_eq!(source(&n.cells()[0]), "café\nnext");
        set_source(&mut n.cells_mut()[0], "café\nend");
        n.save().unwrap();
        let loaded = Notebook::load(&p).unwrap();
        assert_eq!(source(&loaded.cells()[0]), "café\nend");
        assert_eq!(loaded.data["metadata"], v["metadata"]);
        assert_eq!(loaded.cells()[0]["attachments"], json!({"x":{}}));
        fs::write(&p, b"external").unwrap();
        assert!(n.save().is_err());
        assert_eq!(fs::read(&p).unwrap(), b"external");
    }
}
