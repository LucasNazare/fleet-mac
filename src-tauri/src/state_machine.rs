//! Pure session state transitions driven by hook events and PTY exits.
use crate::hooks::{HookEvent, Parsed};
use crate::identity::title_from_prompt;
use crate::model::{Session, SessionState};

#[derive(Debug, Default, PartialEq)]
pub struct Effects {
    pub changed: bool,
    /// The state changed (as opposed to metadata only).
    pub state_changed: bool,
    pub regenerate_title: bool,
    pub refresh_context: bool,
    pub title_changed: bool,
    /// A turn (prompt → Stop) ended: (wall ms, ms of it spent waiting on the user).
    pub turn_done: Option<(i64, i64)>,
}

fn set_state(s: &mut Session, st: SessionState, now: i64, fx: &mut Effects) {
    if s.state != st {
        if s.metrics.turn_started.is_some() && matches!(s.state, SessionState::NeedsPermission | SessionState::NeedsReply) {
            s.metrics.turn_wait += (now - s.state_since).max(0);
        }
        s.state = st;
        s.state_since = now;
        fx.state_changed = true;
    }
    fx.changed = true;
}

fn start_turn(s: &mut Session, now: i64) {
    if s.metrics.turn_started.is_none() {
        s.metrics.turn_started = Some(now);
        s.metrics.turn_wait = 0;
    }
}

fn end_turn(s: &mut Session, now: i64, fx: &mut Effects) {
    let Some(start) = s.metrics.turn_started.take() else { return };
    let m = &mut s.metrics;
    let ms = (now - start).max(0);
    let wait = m.turn_wait.min(ms);
    m.turn_wait = 0;
    m.turns += 1;
    m.turn_ms += ms;
    m.wait_ms += wait;
    m.last_turn_ms = Some(ms);
    m.last_wait_ms = Some(wait);
    fx.turn_done = Some((ms, wait));
    fx.changed = true;
}

pub fn ends_with_question(text: &str) -> bool {
    let t = text.trim_end().trim_end_matches(|c: char| c == '*' || c == '`' || c == '_' || c == ')' || c == '"');
    t.ends_with('?')
}

pub fn apply(s: &mut Session, p: &Parsed, now: i64) -> Effects {
    let mut fx = Effects::default();
    if let Some(id) = p.claude_session_id.as_ref().filter(|id| uuid::Uuid::parse_str(id).is_ok()) {
        if &s.claude_session_id != id {
            s.claude_session_id = id.clone();
            fx.changed = true;
        }
    }
    if let Some(tp) = &p.transcript_path {
        if s.transcript_path.as_ref() != Some(tp) {
            s.transcript_path = Some(tp.clone());
            fx.changed = true;
        }
    }
    if let Some(m) = &p.model {
        s.model = Some(m.clone());
    }
    match &p.event {
        HookEvent::SessionStart { source } => {
            // compact happens mid-task; keep whatever state we had.
            if source != "compact" {
                s.pending_question = None;
                set_state(s, SessionState::Idle, now, &mut fx);
            }
        }
        HookEvent::UserPromptSubmit { prompt } => {
            s.last_prompt = Some(prompt.clone());
            s.pending_question = None;
            if s.title_history.is_empty() && !s.title_locked && s.recap.is_empty() && is_placeholder_title(&s.title) && !prompt.trim().starts_with('/') {
                s.title = title_from_prompt(prompt);
                fx.title_changed = true;
            }
            set_state(s, SessionState::Working, now, &mut fx);
            start_turn(s, now);
        }
        HookEvent::PreToolUse { .. } => {
            s.pending_question = None;
            set_state(s, SessionState::Working, now, &mut fx);
        }
        HookEvent::PostToolUse { file, .. } => {
            s.pending_question = None;
            if let Some(f) = file {
                if !s.files_touched.contains(f) {
                    s.files_touched.push(f.clone());
                }
            }
            fx.refresh_context = true;
            set_state(s, SessionState::Working, now, &mut fx);
        }
        HookEvent::PermissionRequest { question } => {
            s.pending_question = Some(question.clone());
            set_state(s, SessionState::NeedsPermission, now, &mut fx);
        }
        HookEvent::Notification { kind, message } => match kind.as_str() {
            "permission_prompt" => {
                // PermissionRequest usually got here first with a better question.
                if s.state != SessionState::NeedsPermission || s.pending_question.is_none() {
                    s.pending_question = Some(message.clone()).filter(|m| !m.is_empty());
                }
                set_state(s, SessionState::NeedsPermission, now, &mut fx);
            }
            "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                s.pending_question = Some(message.clone()).filter(|m| !m.is_empty());
                set_state(s, SessionState::NeedsReply, now, &mut fx);
            }
            "idle_prompt" => {
                // Claude is at its prompt. If we still think it's busy (e.g. the user
                // pressed Esc, which fires no Stop hook), it's really idle.
                if matches!(s.state, SessionState::Working | SessionState::NeedsPermission) {
                    s.pending_question = None;
                    set_state(s, SessionState::Idle, now, &mut fx);
                    end_turn(s, now, &mut fx);
                }
            }
            _ => {}
        },
        HookEvent::Stop { last_message } => {
            s.pending_question = None;
            if ends_with_question(last_message) {
                let q = last_message.trim().lines().last().unwrap_or("").trim().to_string();
                s.pending_question = Some(q).filter(|q| !q.is_empty());
                set_state(s, SessionState::NeedsReply, now, &mut fx);
            } else {
                set_state(s, SessionState::Done, now, &mut fx);
            }
            end_turn(s, now, &mut fx);
            fx.regenerate_title = true;
            fx.refresh_context = true;
        }
        HookEvent::StopFailure { message } => {
            s.pending_question = Some(message.clone()).filter(|m| !m.is_empty());
            set_state(s, SessionState::Failed, now, &mut fx);
            end_turn(s, now, &mut fx);
        }
        HookEvent::SessionEnd { .. } => {
            if s.state != SessionState::Failed {
                set_state(s, SessionState::Idle, now, &mut fx);
            }
            end_turn(s, now, &mut fx);
        }
        HookEvent::Other(_) => {}
    }
    fx
}

pub fn is_placeholder_title(t: &str) -> bool {
    t == "New session" || t.starts_with("Resumed ")
}

/// The claude process exited.
pub fn on_exit(s: &mut Session, code: Option<i32>, now: i64) -> Effects {
    let mut fx = Effects::default();
    s.alive = false;
    s.exit_code = code;
    s.metrics.turn_started = None; // killed mid-turn: not a finished turn
    let st = if code.unwrap_or(0) != 0 { SessionState::Failed } else { SessionState::Idle };
    s.pending_question = if st == SessionState::Failed { Some(format!("exited with code {}", code.unwrap_or(-1))) } else { None };
    set_state(s, st, now, &mut fx);
    fx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::parse;
    use serde_json::json;

    fn session() -> Session {
        Session {
            id: "s1".into(), project_id: "p".into(), claude_session_id: "c".into(), glyph: "PX".into(),
            color: "#FFA657".into(), title: "New session".into(), title_history: vec![], recap: vec![],
            cwd: "/r".into(), branch: "main".into(), worktree_path: None, state: SessionState::Idle,
            state_since: 0, pending_question: None, context_pct: None, files_touched: vec![], created_at: 0,
            pinned: false, pinned_color: false, pinned_glyph: false, title_locked: false, archived: false,
            permission_mode: None, alive: true, transcript_path: None, last_prompt: None, model: None, exit_code: None,
            line_marks: Default::default(),
            remote: None,
            prompt_count: 0,
            sync: None,
            metrics: Default::default(),
        }
    }

    fn ev(v: serde_json::Value) -> Parsed { parse(&v) }

    #[test]
    fn turn_timing_records_wait() {
        let mut s = session();
        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"go"})), 1_000);
        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"queued"})), 1_500);
        apply(&mut s, &ev(json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"ls"}})), 2_000);
        apply(&mut s, &ev(json!({"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}})), 5_000);
        let fx = apply(&mut s, &ev(json!({"hook_event_name":"Stop","last_assistant_message":"done"})), 9_000);
        assert_eq!(fx.turn_done, Some((8_000, 3_000)));
        assert_eq!((s.metrics.turns, s.metrics.turn_ms, s.metrics.wait_ms, s.metrics.last_turn_ms), (1, 8_000, 3_000, Some(8_000)));
        assert_eq!(s.metrics.turn_started, None);
        // A Stop without a prompt (e.g. after restore) is not a turn.
        let fx = apply(&mut s, &ev(json!({"hook_event_name":"Stop","last_assistant_message":"x"})), 10_000);
        assert_eq!(fx.turn_done, None);
        // Killed mid-turn: dropped.
        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"go"})), 11_000);
        on_exit(&mut s, Some(1), 12_000);
        assert_eq!((s.metrics.turns, s.metrics.turn_started), (1, None));
    }

    #[test]
    fn full_cycle() {
        let mut s = session();
        apply(&mut s, &ev(json!({"hook_event_name":"SessionStart","source":"startup","session_id":"not-a-uuid"})), 1);
        assert_eq!(s.claude_session_id, "c", "non-UUID ids are ignored");
        let real = "4ed8994f-131b-488a-87e0-1c9b101073a6";
        let fx = apply(&mut s, &ev(json!({"hook_event_name":"SessionStart","source":"startup","session_id":real,"transcript_path":"/t"})), 1);
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.claude_session_id, real);
        assert!(fx.changed);

        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"Pix fee labels on checkout"})), 2);
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.state_since, 2);
        assert_eq!(s.title, "Pix fee labels on checkout");

        apply(&mut s, &ev(json!({"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"./deploy.sh all"}})), 3);
        assert_eq!(s.state, SessionState::NeedsPermission);
        assert_eq!(s.pending_question.as_deref(), Some("run ./deploy.sh all?"));

        // The later Notification must not clobber the better question.
        apply(&mut s, &ev(json!({"hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"})), 4);
        assert_eq!(s.pending_question.as_deref(), Some("run ./deploy.sh all?"));
        assert_eq!(s.state_since, 3);

        apply(&mut s, &ev(json!({"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":"/r/a.ts"}})), 5);
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.files_touched, vec!["/r/a.ts".to_string()]);
        assert!(s.pending_question.is_none());

        let fx = apply(&mut s, &ev(json!({"hook_event_name":"Stop","last_assistant_message":"All done."})), 6);
        assert_eq!(s.state, SessionState::Done);
        assert!(fx.regenerate_title);

        // Second prompt doesn't overwrite title.
        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"now something else"})), 7);
        assert_eq!(s.title, "Pix fee labels on checkout");
    }

    #[test]
    fn stop_with_question_needs_reply() {
        let mut s = session();
        apply(&mut s, &ev(json!({"hook_event_name":"Stop","last_assistant_message":"I found two options.\n\nShould I use **A or B?**"})), 1);
        assert_eq!(s.state, SessionState::NeedsReply);
        assert_eq!(s.pending_question.as_deref(), Some("Should I use **A or B?**"));
    }

    #[test]
    fn idle_prompt_unsticks_working() {
        let mut s = session();
        s.state = SessionState::Working;
        apply(&mut s, &ev(json!({"hook_event_name":"Notification","notification_type":"idle_prompt"})), 1);
        assert_eq!(s.state, SessionState::Idle);
        s.state = SessionState::Done;
        apply(&mut s, &ev(json!({"hook_event_name":"Notification","notification_type":"idle_prompt"})), 2);
        assert_eq!(s.state, SessionState::Done);
    }

    #[test]
    fn exit_codes() {
        let mut s = session();
        on_exit(&mut s, Some(0), 1);
        assert_eq!(s.state, SessionState::Idle);
        on_exit(&mut s, Some(1), 2);
        assert_eq!(s.state, SessionState::Failed);
        assert!(!s.alive);
    }

    #[test]
    fn slash_command_doesnt_title() {
        let mut s = session();
        apply(&mut s, &ev(json!({"hook_event_name":"UserPromptSubmit","prompt":"/compact"})), 1);
        assert_eq!(s.title, "New session");
    }
}
