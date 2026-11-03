//! Port of agents/opencode-questions.ts: a question request from OpenCode's API, answered one
//! question at a time the way its terminal does.

use std::collections::BTreeSet;

use inyon_protocol::{PromptOption, PromptTab, ScreenPrompt, CONFIRM_OPTION};
use serde::Deserialize;

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// One question of OpenCode's question tool, as its `/question` route lists them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OpenCodeQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: Option<bool>,
    /// a typed answer is allowed (the default)
    #[serde(default)]
    pub custom: Option<bool>,
}

const FREE_TEXT: &str = "Type your own answer";

/// A single choice moves on to the next question, checkboxes are toggled and sent with Confirm,
/// and the last answer sends them all (`answers`, one list of labels per question). The fields
/// are read-only from outside, as the TS class's are: the per-question state is sized to the
/// questions when the flow starts.
///
/// ```compile_fail
/// use inyon_core::agents::opencode_questions::OpenCodeQuestionFlow;
/// // replacing the questions would leave the checkboxes sized for the old ones
/// fn clear(flow: &mut OpenCodeQuestionFlow) {
///     flow.questions.clear();
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OpenCodeQuestionFlow {
    id: String,
    questions: Vec<OpenCodeQuestion>,
    index: usize,
    answers: Vec<Vec<String>>,
    checked: Vec<BTreeSet<u32>>,
    typed: Vec<Option<String>>,
}

impl OpenCodeQuestionFlow {
    pub fn new(id: impl Into<String>, questions: Vec<OpenCodeQuestion>) -> Result<Self, String> {
        if questions.is_empty() {
            return Err("OpenCode asked nothing".into());
        }
        let n = questions.len();
        Ok(Self { id: id.into(), questions, index: 0, answers: vec![], checked: vec![BTreeSet::new(); n], typed: vec![None; n] })
    }

    /// the request id the answers go back under
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn questions(&self) -> &[OpenCodeQuestion] {
        &self.questions
    }
    /// which question is open
    pub fn index(&self) -> usize {
        self.index
    }
    /// the labels chosen so far, one list per question answered
    pub fn answers(&self) -> &[Vec<String>] {
        &self.answers
    }

    pub fn done(&self) -> bool {
        self.index >= self.questions.len()
    }

    /// The open question as a prompt the app can show.
    pub fn prompt(&self) -> ScreenPrompt {
        let q = &self.questions[self.index.min(self.questions.len() - 1)];
        let multi = q.multiple.unwrap_or(false);
        let checked = self.checked.get(self.index);
        let mut options: Vec<PromptOption> = q
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| PromptOption {
                n: i as u64 + 1,
                label: o.label.clone(),
                detail: o.description.clone().filter(|d| !d.is_empty() && *d != o.label),
                selected: i == 0,
                checked: multi.then(|| checked.is_some_and(|c| c.contains(&(i as u32 + 1)))),
                current: None,
            })
            .collect();
        let custom = q.custom != Some(false);
        if custom {
            let typed = self.typed.get(self.index).is_some_and(Option::is_some);
            options.push(PromptOption { n: options.len() as u64 + 1, label: FREE_TEXT.into(), detail: None, selected: false, checked: multi.then_some(typed), current: None });
        }
        let more = self.index + 1 < self.questions.len();
        ScreenPrompt {
            body: vec![q.question.clone()],
            whole_lines: None,
            free_text_option: custom.then_some(options.len() as u64),
            options,
            tabs: (self.questions.len() > 1)
                .then(|| self.questions.iter().enumerate().map(|(i, x)| PromptTab { label: x.header.clone(), done: i < self.index, current: i == self.index }).collect()),
            footer: None,
            multi,
            navigate: None,
            input: None,
            links: None,
            api: Some(true),
            confirm: multi.then(|| if more { "Next" } else { "Confirm" }.into()),
        }
    }

    /// Applies a tap: an option number (toggled when the question takes several), the free-text
    /// option with what was typed, or CONFIRM_OPTION to send the toggled choices. True once the
    /// last question is answered and `answers` is ready to send.
    pub fn answer(&mut self, option: u32, text: Option<&str>) -> Result<bool, String> {
        if self.done() {
            return Err("These questions were answered already".into());
        }
        let q = &self.questions[self.index];
        let free = (q.custom != Some(false)).then_some(q.options.len() as u32 + 1);
        let typed_now = text.map(|t| crate::js::JsStr::jtrim(t).to_owned());
        let multiple = q.multiple.unwrap_or(false);
        if option == CONFIRM_OPTION {
            if !multiple {
                return Err("This question takes one answer".into());
            }
            let mut picked: Vec<String> = self.checked[self.index].iter().map(|n| q.options[*n as usize - 1].label.clone()).collect();
            if let Some(t) = self.typed[self.index].clone().filter(|t| !t.is_empty()) {
                picked.push(t);
            }
            if picked.is_empty() {
                return Err("Pick at least one answer".into());
            }
            return Ok(self.advance(picked));
        }
        let is_free = Some(option) == free;
        if !is_free && q.options.get(option as usize - 1).is_none() {
            return Err("That choice is not in the question".into());
        }
        if is_free && typed_now.as_deref().is_none_or(str::is_empty) {
            return Err("Type an answer first".into());
        }
        if !multiple {
            let chosen = if is_free { typed_now.unwrap_or_default() } else { q.options[option as usize - 1].label.clone() };
            return Ok(self.advance(vec![chosen]));
        }
        if is_free {
            let slot = &mut self.typed[self.index];
            *slot = if *slot == typed_now { None } else { typed_now };
        } else if !self.checked[self.index].remove(&option) {
            self.checked[self.index].insert(option);
        }
        Ok(false)
    }

    fn advance(&mut self, answer: Vec<String>) -> bool {
        self.answers.push(answer);
        self.index += 1;
        self.done()
    }
}

#[cfg(test)]
#[path = "opencode_questions_tests.rs"]
mod tests;
