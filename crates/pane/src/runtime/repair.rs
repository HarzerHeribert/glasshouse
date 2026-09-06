//! A narrowly scoped edit of the most recent cell that failed to parse.

use serde::Deserialize;

pub const SOURCE_BYTE_CAP: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxFailure {
    pub cell: u64,
    pub source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    cell: u64,
    replace: String,
    with: String,
}

impl SyntaxFailure {
    pub fn new(cell: u64, source: &str) -> Option<Self> {
        (source.len() <= SOURCE_BYTE_CAP).then(|| Self {
            cell,
            source: source.to_string(),
        })
    }

    pub fn apply(&self, json: &str) -> Result<String, String> {
        if json.len() > SOURCE_BYTE_CAP {
            return Err("pane-edit exceeds the 128 KiB limit".into());
        }
        let edit: Edit = serde_json::from_str(json)
            .map_err(|error| format!("invalid pane-edit JSON: {error}"))?;
        if edit.cell != self.cell {
            return Err(format!(
                "stale pane-edit: cell {} is not the eligible cell {}",
                edit.cell, self.cell
            ));
        }
        if edit.replace.is_empty() {
            return Err("pane-edit `replace` must be nonempty".into());
        }
        let Some(start) = self.source.find(&edit.replace) else {
            return Err("pane-edit `replace` does not occur in the failed source".into());
        };
        // Count overlapping matches too: "aa" is ambiguous inside "aaa".
        let next = start + edit.replace.chars().next().expect("nonempty").len_utf8();
        if self.source[next..].contains(&edit.replace) {
            return Err("pane-edit `replace` must occur exactly once in the failed source".into());
        }
        let end = start + edit.replace.len();
        let resulting_len = self.source.len() - edit.replace.len() + edit.with.len();
        if resulting_len > SOURCE_BYTE_CAP {
            return Err("amended source exceeds the 128 KiB limit".into());
        }
        let mut amended = String::with_capacity(resulting_len);
        amended.push_str(&self.source[..start]);
        amended.push_str(&edit.with);
        amended.push_str(&self.source[end..]);
        if amended == self.source {
            return Err("pane-edit must change the failed source".into());
        }
        Ok(amended)
    }

    pub fn hint(&self) -> String {
        format!(
            "Nothing in cell {} ran. Amend its source with one fence:\n```pane-edit\n{{\"cell\":{},\"replace\":\"exact text occurring once\",\"with\":\"replacement\"}}\n```",
            self.cell, self.cell
        )
    }
}
