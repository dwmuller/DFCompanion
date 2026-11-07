use std::{fmt::Write as _, fs, io, path::PathBuf};

use crate::project_dir;
use chrono::Local;

pub fn write_ai_failure_dump(
    reason: &str,
    adapter: &str,
    model: &str,
    request: &genai::chat::ChatRequest,
    response: &genai::chat::ChatResponse,
) -> io::Result<PathBuf> {
    let dir = project_dir().data_local_dir().join("diagnostics");
    fs::create_dir_all(&dir)?;

    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S%.3f");
    let path = dir.join(format!("ai-failure-{timestamp}.txt"));

    let mut dump = String::new();

    writeln!(dump, "DFCompanion AI failure diagnostic").unwrap();
    writeln!(dump, "Time: {}", Local::now().to_rfc3339()).unwrap();
    writeln!(dump, "Reason: {reason}").unwrap();
    writeln!(dump, "Adapter: {adapter}").unwrap();
    writeln!(dump, "Model: {model}").unwrap();

    writeln!(dump, "\nREQUEST\n=======").unwrap();
    writeln!(dump, "{request:#?}").unwrap();

    writeln!(dump, "\nRESPONSE\n========").unwrap();
    writeln!(dump, "{response:#?}").unwrap();

    fs::write(&path, dump)?;

    Ok(path)
}
