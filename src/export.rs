//! Dependency-free transcript exporters.

use std::{fs, path::Path};

use anyhow::{Result, bail};

use crate::model::ChatState;

pub fn render(state: &ChatState, format: &str) -> Result<String> {
    match format.to_ascii_lowercase().as_str() {
        "markdown" | "md" => Ok(markdown(state)),
        "text" | "txt" => Ok(text(state)),
        "json" => Ok(serde_json::to_string_pretty(&state.history)?),
        "html" => Ok(html(state)),
        "pdf" => Ok(pdf(state)),
        value => bail!("unsupported export format '{value}'"),
    }
}

pub fn write(state: &ChatState, format: &str, path: &Path) -> Result<()> {
    let rendered = render(state, format)?;
    fs::write(path, rendered)?;
    Ok(())
}

fn text(state: &ChatState) -> String {
    let mut output = format!("r105 Conversation\nMessages: {}\n\n", state.history.len());
    for (index, message) in state.history.iter().enumerate() {
        output.push_str(&format!(
            "[{}] {}\n{}\n\n",
            index + 1,
            message.role.to_ascii_uppercase(),
            message.content
        ));
    }
    output
}

fn markdown(state: &ChatState) -> String {
    let mut output = format!(
        "# r105 Conversation\n\nMessages: {}\n\n",
        state.history.len()
    );
    for (index, message) in state.history.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {}\n\n{}\n\n---\n\n",
            index + 1,
            message.role.to_ascii_uppercase(),
            message.content
        ));
        if !message.tool_calls.is_empty() {
            output.push_str("Tool calls:\n\n");
            output.push_str(&serde_json::to_string_pretty(&message.tool_calls).unwrap_or_default());
            output.push_str("\n\n");
        }
    }
    output
}

fn html(state: &ChatState) -> String {
    let messages = state
        .history
        .iter()
        .map(|message| {
            format!(
                "<article class=\"message {}\"><header>{}</header><pre>{}</pre></article>",
                escape(&message.role),
                escape(&message.role.to_ascii_uppercase()),
                escape(&message.content)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>r105 Conversation</title>\
         <style>body{{font:16px system-ui;max-width:900px;margin:2rem auto;background:#10131a;color:#e5e7eb}}\
         .message{{padding:1rem;margin:1rem 0;border-left:4px solid #7dd3fc;background:#1f2937}}\
         pre{{white-space:pre-wrap;font:14px ui-monospace,monospace}}</style></head>\
         <body><h1>r105 Conversation</h1><p>{}</p>{}</body></html>",
        state.history.len(),
        messages
    )
}

fn pdf(state: &ChatState) -> String {
    let mut output = String::from("%PDF-1.4\n");
    let body = format!("r105 Conversation - {} messages", state.history.len());
    let stream = format!("BT /F1 10 Tf 40 760 Td ({}) Tj ET", pdf_escape(&body));
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
    ];
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
    }
    let start = output.len();
    output.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets.iter().skip(1) {
        output.push_str(&format!("{offset:010} 00000 n \n"));
    }
    output.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        objects.len() + 1,
        start
    ));
    output
}

fn pdf_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
