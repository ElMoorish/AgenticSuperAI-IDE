//! Plan / Build state machine with mode-gated tool lists (Doc A section D.1).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Plan,
    Build,
}

impl Mode {
    /// Keep tool lists tiny: small models fail when given too many tools.
    /// Only tools that are actually implemented are listed here.
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Mode::Plan => &["list_dir", "find_files", "search_text", "read_file"],
            Mode::Build => &["list_dir", "find_files", "search_text", "read_file", "edit_file_search_replace", "run_compiler_check", "run_shell_scoped"],
        }
    }

    fn intro(self) -> &'static str {
        match self {
            Mode::Plan => "You are in PLAN mode. Read code and produce a short Markdown plan. Do not edit files. If you do not know the project layout, call list_dir with path \".\" first. Never guess file names: use list_dir, find_files or search_text.",
            Mode::Build => "You are in BUILD mode. Apply changes with small SEARCH/REPLACE edits. If you do not know the project layout, call list_dir with path \".\" first. Never guess file names or the language: look first. Read the file before editing it, then run_compiler_check after editing and fix any errors.",
        }
    }

    /// `native`: tools are sent through the API's `tools` field, so the prompt only needs the intro.
    /// Otherwise the tool format and list are spelled out in the prompt (XML fallback).
    pub fn system_prompt(self, native: bool) -> String {
        if native {
            return format!("{}\n\nUse the provided tools. When the task is done, answer in plain text without calling a tool.", self.intro());
        }
        let mut s = format!("{}\n\nTo use a tool, reply with exactly one block and nothing after it:\n<tool_call>{{\"name\":\"read_file\",\"arguments\":{{\"path\":\"README.md\"}}}}</tool_call>\n(that is an example; use the tool names listed below). You will receive a <tool_result>. When done, answer in plain text without a tool call.\n\nTools:\n", self.intro());
        for t in self.tools() {
            s.push_str(match *t {
                "list_dir" => "- list_dir {\"path\": \".\"} (lists a folder; \".\" is the project root)\n",
                "find_files" => "- find_files {\"query\": \"part of a file name\"}\n",
                "search_text" => "- search_text {\"query\": \"text to find inside files\"}\n",
                "read_file" => "- read_file {\"path\": \"relative/path\"}\n",
                "edit_file_search_replace" => "- edit_file_search_replace {\"path\": \"relative/path\", \"diff\": \"<<<<<<< SEARCH\\n:start_line:N\\nold lines\\n=======\\nnew lines\\n>>>>>>> REPLACE\"} (:start_line: optional)\n",
                "run_compiler_check" => "- run_compiler_check {} (auto-detects cargo / tsc / cmake / luacheck; optional \"command\" to override)\n",
                "run_shell_scoped" => "- run_shell_scoped {\"command\": \"program arg1 arg2\"} (no pipes, redirects or chaining)\n",
                _ => "",
            });
        }
        s
    }

    /// OpenAI-style function schemas for this mode's tools.
    pub fn tool_schemas(self) -> Vec<Value> {
        let f = |name: &str, desc: &str, props: Value, required: Value| {
            json!({"type": "function", "function": {"name": name, "description": desc,
                   "parameters": {"type": "object", "properties": props, "required": required}}})
        };
        self.tools()
            .iter()
            .map(|t| match *t {
                "list_dir" => f("list_dir", "List the files and folders in a directory. Use path \".\" for the project root. Folders end with /.", json!({"path": {"type": "string", "description": "Relative folder path, \".\" for the root"}}), json!([])),
                "find_files" => f("find_files", "Find files by part of their name or path.", json!({"query": {"type": "string"}}), json!(["query"])),
                "search_text" => f("search_text", "Search inside project files for text. Returns path:line: text.", json!({"query": {"type": "string"}}), json!(["query"])),
                "read_file" => f("read_file", "Read a text file. Path is relative to the project root.", json!({"path": {"type": "string"}}), json!(["path"])),
                "edit_file_search_replace" => f(
                    "edit_file_search_replace",
                    "Edit a file with one or more SEARCH/REPLACE blocks. Each block is: a line `<<<<<<< SEARCH`, the exact old lines, a line `=======`, the new lines, a line `>>>>>>> REPLACE`.",
                    json!({"path": {"type": "string"}, "diff": {"type": "string", "description": "One or more SEARCH/REPLACE blocks"}}),
                    json!(["path", "diff"]),
                ),
                "run_compiler_check" => f(
                    "run_compiler_check",
                    "Compile/check the project (auto-detects cargo, tsc, cmake or luacheck) and return errors.",
                    json!({"command": {"type": "string", "description": "Optional override, e.g. `cargo check`"}}),
                    json!([]),
                ),
                _ => f(
                    "run_shell_scoped",
                    "Run one command (program and arguments only: no pipes, redirects or chaining).",
                    json!({"command": {"type": "string"}}),
                    json!(["command"]),
                ),
            })
            .collect()
    }
}

pub fn tool_allowed(mode: Mode, tool: &str) -> bool {
    mode.tools().contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_cannot_edit() {
        assert!(!tool_allowed(Mode::Plan, "edit_file_search_replace"));
        assert!(tool_allowed(Mode::Plan, "read_file"));
    }

    #[test]
    fn build_mode_can_edit() {
        assert!(tool_allowed(Mode::Build, "edit_file_search_replace"));
        assert!(!tool_allowed(Mode::Build, "rm_rf"));
    }

    #[test]
    fn xml_prompt_lists_only_allowed_tools() {
        assert!(!Mode::Plan.system_prompt(false).contains("edit_file_search_replace"));
        assert!(Mode::Build.system_prompt(false).contains("edit_file_search_replace"));
    }

    #[test]
    fn native_prompt_has_no_xml_format() {
        assert!(!Mode::Build.system_prompt(true).contains("<tool_call>"));
    }

    #[test]
    fn schemas_match_mode_tools() {
        let names = |m: Mode| -> Vec<String> {
            m.tool_schemas().iter().map(|s| s["function"]["name"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(names(Mode::Plan), vec!["read_file"]);
        assert_eq!(names(Mode::Build), Mode::Build.tools());
    }
}
