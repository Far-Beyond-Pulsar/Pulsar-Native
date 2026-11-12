//! The engine's startup prompts, shown by the `pulsar-dialog` helper.
//!
//! These run before the editor has a window, so they go through the helper
//! (`pulsar_dialog_protocol::client`) rather than the engine's own UI. If the
//! helper is missing the platform's message box is used instead, so a prompt
//! is never lost. Every prompt that used to be an `rfd::MessageDialog` is a
//! call to [`notify`] or [`confirm`] here.

use pulsar_dialog_protocol::{
    client, ButtonKind, DialogButton, DialogRequest, DialogResponse, Outcome, Severity,
};

const YES: &str = "yes";
const NO: &str = "no";

/// Tell the user something. Returns when it is dismissed.
pub fn notify(title: &str, message: impl Into<String>, severity: Severity, detail: Option<String>) {
    let mut request = DialogRequest::new(title, message).severity(severity).ok();
    if let Some(detail) = detail {
        request = request.detail(detail);
    }
    let _ = client::show(&request);
}

/// Ask a yes/no question: `yes` is the filled default button (Enter), `no` is
/// Escape and closing the window. `if_unavailable` is the answer when nothing
/// could show the dialog: a prompt that guards a download says no, one that
/// only offers a convenience may say no too.
pub fn confirm(
    title: &str,
    message: impl Into<String>,
    severity: Severity,
    yes: &str,
    no: &str,
    if_unavailable: bool,
) -> bool {
    let request = DialogRequest::new(title, message)
        .severity(severity)
        .button(DialogButton::new(YES, yes).kind(ButtonKind::Primary).default())
        .button(DialogButton::new(NO, no).cancel());
    answer_is_yes(&client::show(&request), if_unavailable)
}

/// Whether an answer to a [`confirm`] is yes. Closing the dialog is no; no
/// dialog at all is `if_unavailable`.
fn answer_is_yes(answer: &DialogResponse, if_unavailable: bool) -> bool {
    match answer.outcome {
        Outcome::Unavailable => if_unavailable,
        Outcome::Dismissed => false,
        Outcome::Answered => answer.is(YES),
    }
}

#[cfg(test)]
mod tests {
    use super::{answer_is_yes, NO, YES};
    use pulsar_dialog_protocol::DialogResponse;

    #[test]
    fn only_the_yes_button_is_yes() {
        assert!(answer_is_yes(&DialogResponse::answered(YES, None), false));
        assert!(!answer_is_yes(&DialogResponse::answered(NO, None), true));
    }

    #[test]
    fn closing_the_dialog_is_no_and_silence_takes_the_callers_default() {
        assert!(!answer_is_yes(&DialogResponse::dismissed(None), true));
        assert!(answer_is_yes(&DialogResponse::unavailable(), true));
        assert!(!answer_is_yes(&DialogResponse::unavailable(), false));
    }
}
