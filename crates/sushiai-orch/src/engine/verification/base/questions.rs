use crate::engine::verification::base::answers::short7;
use crate::engine::verification::base::answers::PARENT_CHECK_OPTIONS;
use crate::engine::verification::base::answers::PRE_EXISTING_OPTIONS;
use crate::prompts::text::tail_chars;
use crate::task::attempt::VerifyOutcome;
use crate::task::questions::AskedBy;
use crate::task::questions::Question;
use crate::task::questions::QuestionKind;

/// The one owner question for every check that fails on `base_sha`: what
/// each of them printed is quoted, the commands are listed in `commands`.
fn base_check_question(failed: &[VerifyOutcome], base_sha: &str, options: [&str; 3]) -> Question {
    let sha = short7(base_sha);
    let text = match failed {
        [one] => format!(
            "{} already fails on base {sha}: {}",
            one.command,
            tail_chars(one.tail.trim(), 300)
        ),
        many => {
            let names: Vec<&str> = many.iter().map(|f| f.command.as_str()).collect();
            let mut text = format!("{} already fail on base {sha}:", names.join(", "));
            for f in many {
                text.push_str(&format!(
                    "\n- {}: {}",
                    f.command,
                    tail_chars(f.tail.trim(), 200)
                ));
            }
            text
        }
    };
    let mut q = Question::new(
        text,
        options.iter().map(|o| o.to_string()).collect(),
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
        crate::clock::now_ms(),
    );
    q.commands = failed.iter().map(|f| f.command.clone()).collect();
    q.base_tails = failed.iter().map(|f| f.tail.trim().to_string()).collect();
    q
}

/// The owner question for final checks that fail on the base as well.
pub(in crate::engine) fn pre_existing_question(
    failed: &[VerifyOutcome],
    base_sha: &str,
) -> Question {
    base_check_question(failed, base_sha, PRE_EXISTING_OPTIONS)
}

/// The owner question for a parent's checks that fail on the base: unlike a
/// final check on its own, a `verify` command is usually meant to turn green
/// through the work.
pub(in crate::engine) fn parent_checks_question(
    failed: &[VerifyOutcome],
    base_sha: &str,
) -> Question {
    let mut q = base_check_question(failed, base_sha, PARENT_CHECK_OPTIONS);
    let (them, all) = if failed.len() == 1 {
        ("it", "this check")
    } else {
        ("them", "all")
    };
    q.text = format!(
        "{} The subtasks may be meant to make {them} pass: keep {all}, or drop {all}?",
        q.text.trim_end()
    );
    q
}
