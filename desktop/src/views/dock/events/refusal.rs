use api::graph::errors::Refusal;
use serde_json::Value;

use crate::strings::{Lang, Str};

fn text(value: &Value) -> String {
    match value {
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join(", "),
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn args(value: Option<&Value>) -> Vec<(String, String)> {
    match value {
        Some(Value::Object(fields)) => fields.iter().map(|(k, v)| (k.clone(), text(v))).collect(),
        Some(other) => vec![("value".to_owned(), text(other))],
        None => Vec::new(),
    }
}

fn template(code: &str) -> Option<Str> {
    Some(match code {
        "emptyName" => Str::ProblemRefusedEmptyName,
        "nameTaken" => Str::ProblemRefusedNameTaken,
        "reservedName" => Str::ProblemRefusedReservedName,
        "tooDeep" => Str::ProblemRefusedTooDeep,
        "tooManyChildren" => Str::ProblemRefusedTooManyChildren,
        "briefTooLong" => Str::ProblemRefusedBriefTooLong,
        "summaryTooLong" => Str::ProblemRefusedSummaryTooLong,
        "mailboxFull" => Str::ProblemRefusedMailboxFullNameless,
        "childrenStay" => Str::ProblemRefusedChildrenStay,
        "alreadyRoot" => Str::ProblemRefusedAlreadyRoot,
        "ended" => Str::ProblemRefusedEndedNameless,
        "heldFull" => Str::ProblemRefusedHeldFull,
        "cycle" => Str::ProblemRefusedCycle,
        "notFound" => Str::ProblemRefusedNotFoundNameless,
        "ambiguous" => Str::ProblemRefusedAmbiguousNameless,
        "notSteerable" => Str::ProblemRefusedNotSteerableNameless,
        "noResume" => Str::ProblemRefusedNoResume,
        "noLink" => Str::ProblemRefusedNoLink,
        "relaunching" => Str::ProblemRefusedRelaunching,
        "noRepo" => Str::ProblemRefusedNoRepo,
        "emptyQuestion" => Str::ProblemRefusedEmptyQuestion,
        "noAsked" => Str::ProblemRefusedNoAsked,
        "notAsk" => Str::ProblemRefusedNotAsk,
        "docHasParent" => Str::ProblemRefusedDocHasParent,
        "rolesHaveParent" => Str::ProblemRefusedRolesHaveParent,
        "badRoleSetting" => Str::ProblemRefusedBadRoleSetting,
        "bareLink" => Str::ProblemRefusedBareLink,
        "badChoices" => Str::ProblemRefusedBadChoices,
        "uncheckedDoc" => Str::ProblemRefusedUncheckedDoc,
        "docLinkLabel" => Str::ProblemRefusedDocLinkLabel,
        "notImage" => Str::ProblemRefusedNotImage,
        "unknownRole" => Str::ProblemRefusedUnknownRole,
        "roleHasNoAgent" => Str::ProblemRefusedRoleHasNoAgent,
        "leadChildNeedsWorktree" => Str::ProblemRefusedLeadChildNeedsWorktree,
        "reviewerSeated" => Str::ProblemRefusedReviewerSeated,
        "publishing" => Str::ProblemRefusedPublishing,
        "noWait" => Str::ProblemRefusedNoWait,
        "emptyAnswer" => Str::ProblemRefusedEmptyAnswer,
        "severalOpen" => Str::ProblemRefusedSeveralOpen,
        "promptChanged" => Str::ProblemRefusedPromptChanged,
        "gate" => Str::ProblemRefusedGate,
        "notGate" => Str::ProblemRefusedNotGate,
        "noRound" => Str::ProblemRefusedNoRound,
        "notOwner" => Str::ProblemRefusedNotOwner,
        "uncommitted" => Str::ProblemRefusedUncommitted,
        "sameAsTarget" => Str::ProblemRefusedSameAsTarget,
        "sameHead" => Str::ProblemRefusedSameHead,
        "staleHead" => Str::ProblemRefusedStaleHead,
        "noFindings" => Str::ProblemRefusedNoFindings,
        "kindWord" => Str::ProblemRefusedKindWordNameless,
        "ownVerdict" => Str::ProblemRefusedOwnVerdict,
        "notReviewer" => Str::ProblemRefusedNotReviewer,
        "coordinatorAccepts" => Str::ProblemRefusedCoordinatorAccepts,
        "nothingToMerge" => Str::ProblemRefusedNothingToMerge,
        "git" => Str::ProblemRefusedGit,
        "launch" => Str::ProblemRefusedLaunch,
        "db" => Str::ProblemRefusedDb,
        _ => return None,
    })
}

pub fn nameless(reason: &Refusal, lang: Lang) -> String {
    let json = serde_json::to_value(reason).unwrap_or(Value::Null);
    let code = json.get("code").and_then(Value::as_str).unwrap_or_default();
    let Some(template) = template(code) else {
        return code.to_owned();
    };
    let pairs = args(json.get("value"));
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    template.with(lang, &pairs)
}
