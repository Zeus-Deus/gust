//! What a MIME type is a kind of.
//!
//! The desktop resolves an application for a type from the specific to the
//! general: a `.patch` file is `text/x-patch`, which is a kind of
//! `text/plain`, so an editor associated only with `text/plain` opens it.
//! Anything that asks "is there an application for this file?" has to walk
//! the same line, or it reports a configured desktop as having none.
//!
//! The parents come from the shared MIME database's `subclasses` files
//! (`child parent` per line) plus the two rules the specification states
//! without listing: every `text/*` type is a kind of `text/plain`, and every
//! type that is a stream of bytes is a kind of `application/octet-stream`.

use std::path::PathBuf;

/// More types than any real line of descent has; a cyclic or hostile
/// database stops here instead of growing without end.
const LIMIT: usize = 32;

/// `mime`, then every type it is a kind of, nearest first.
///
/// `subclasses` is the text of each `subclasses` file, the user's first.
/// Pure: it reads no file.
pub fn lineage(mime: &str, subclasses: &[String]) -> Vec<String> {
    let mut out = vec![mime.to_string()];
    let mut next = 0;
    while next < out.len() && out.len() < LIMIT {
        let current = out[next].clone();
        next += 1;
        let listed = subclasses
            .iter()
            .flat_map(|text| text.lines())
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(current.as_str()))
                    .then(|| words.next())
                    .flatten()
            });
        let implied =
            (current.starts_with("text/") && current != "text/plain").then_some("text/plain");
        for parent in listed.chain(implied) {
            if out.len() < LIMIT && !out.iter().any(|seen| seen == parent) {
                out.push(parent.to_string());
            }
        }
    }
    // Folders and other non-files are not streams of bytes.
    let stream = "application/octet-stream";
    if !mime.starts_with("inode/") && !out.iter().any(|seen| seen == stream) {
        out.push(stream.to_string());
    }
    out
}

/// [`lineage`] from the shared MIME database on this machine: `mime/subclasses`
/// beside each of the given `applications` directories.
pub fn lineage_beside(mime: &str, application_dirs: &[PathBuf]) -> Vec<String> {
    let texts: Vec<String> = application_dirs
        .iter()
        .filter_map(|dir| dir.parent())
        .filter_map(|data| std::fs::read_to_string(data.join("mime/subclasses")).ok())
        .collect();
    lineage(mime, &texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Vec<String> {
        vec![
            "text/x-patch text/plain\napplication/xhtml+xml application/xml\n".to_string(),
            "application/xml text/plain\nimage/svg+xml application/xml\n".to_string(),
        ]
    }

    #[test]
    fn a_text_type_is_a_kind_of_plain_text_even_when_no_file_says_so() {
        // The case that matters: text/x-diff has no line in the database,
        // and an editor associated only with text/plain still opens it.
        assert_eq!(
            lineage("text/x-diff", &db()),
            ["text/x-diff", "text/plain", "application/octet-stream"]
        );
    }

    #[test]
    fn listed_parents_are_followed_nearest_first_across_files() {
        assert_eq!(
            lineage("image/svg+xml", &db()),
            [
                "image/svg+xml",
                "application/xml",
                "text/plain",
                "application/octet-stream"
            ]
        );
    }

    #[test]
    fn plain_text_and_bytes_are_not_their_own_parents() {
        assert_eq!(
            lineage("text/plain", &db()),
            ["text/plain", "application/octet-stream"]
        );
        assert_eq!(
            lineage("application/octet-stream", &db()),
            ["application/octet-stream"]
        );
    }

    #[test]
    fn a_folder_is_not_a_stream_of_bytes() {
        assert_eq!(lineage("inode/directory", &db()), ["inode/directory"]);
    }

    #[test]
    fn a_database_that_loops_ends() {
        let looped = vec!["a/x a/y\na/y a/x\n".to_string()];
        assert_eq!(
            lineage("a/x", &looped),
            ["a/x", "a/y", "application/octet-stream"]
        );
        let endless: String = (0..200).map(|i| format!("b/{i} b/{}\n", i + 1)).collect();
        assert!(lineage("b/0", &[endless]).len() <= LIMIT + 1);
    }

    #[test]
    fn the_database_is_read_beside_the_applications_directories() {
        let root = std::env::temp_dir().join(format!("gust-mimeparents-{}", std::process::id()));
        let data = root.join("share");
        std::fs::create_dir_all(data.join("mime")).unwrap();
        std::fs::create_dir_all(data.join("applications")).unwrap();
        std::fs::write(data.join("mime/subclasses"), "model/vrml text/plain\n").unwrap();
        let dirs = [data.join("applications"), root.join("missing/applications")];
        assert_eq!(
            lineage_beside("model/vrml", &dirs),
            ["model/vrml", "text/plain", "application/octet-stream"]
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
