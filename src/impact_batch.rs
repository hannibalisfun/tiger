//! Prepare the queries before loading definitions, then export their resolutions after validation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Everything;

#[derive(Deserialize)]
struct Manifest {
    layer: String,
    files: Vec<Value>,
}

#[derive(Debug)]
pub struct ImpactBatch {
    diff: Value,
    queries: Vec<String>,
}

impl ImpactBatch {
    pub fn prepare(path: &Path, data: &mut Everything) -> Result<Self> {
        let mut manifest: Manifest = serde_json::from_reader(File::open(path)?)?;
        let mut sources = BTreeMap::new();
        for file in &mut manifest.files {
            let relative = file["path"].as_str().context("Missing impact input path")?.to_owned();
            let path = Path::new(&relative);
            ensure!(
                !relative.is_empty()
                    && path.components().all(|p| matches!(p, Component::Normal(_))),
                "Expected a game-relative path in the impact input: {relative}"
            );
            let mask = data.lookup_file_mask(&manifest.layer, path);
            let mut source = json!({"before_from": manifest.layer, "after_from": manifest.layer,
                                    "file_mask": mask});
            // Adding or deleting a file at the same path can mask or reveal *different*
            // identifiers. Compare the displaced file on that side, using Tiger's load order.
            if mask.is_none()
                && (file["before"].is_null() || file["after"].is_null())
                && let Some((label, lower)) = data.lookup_lower_file(&manifest.layer, path)
            {
                let (side, origin) = if file["before"].is_null() {
                    ("before", "before_from")
                } else {
                    ("after", "after_from")
                };
                file[side] = json!(lower);
                source[origin] = json!(label);
            }
            sources.insert(relative, source);
        }
        let mut diff = crate::impact::changed_symbols(&Value::Array(manifest.files))?;
        let mut names = BTreeSet::new();
        for change in diff["changes"].as_array_mut().context("Expected changed symbols")? {
            let path = change["path"].as_str().context("Changed symbol without a path")?;
            let source = sources.get(path).context("Unknown impact source")?;
            for key in ["before_from", "after_from", "file_mask"] {
                change[key] = source[key].clone();
            }
            names.insert(change["symbol"].as_str().context("Missing changed symbol")?.to_owned());
        }
        let queries = names.into_iter().collect::<Vec<_>>();
        data.set_lookup_queries(&queries);
        data.enable_unresolved_callers();
        Ok(Self { diff, queries })
    }

    pub fn write(&self, path: &Path, data: &Everything) -> Result<()> {
        let mut output = BufWriter::new(File::create_new(path)?);
        let lookups = data.lookup_results(&self.queries);
        serde_json::to_writer(&mut output, &json!({"diff": self.diff, "lookups": lookups}))?;
        output.flush()?;
        Ok(())
    }
}
