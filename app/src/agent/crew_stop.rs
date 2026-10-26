// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stop and Pause on a running crew, end to end: fake brains, fake Codex
//! programs with children of their own, long fake tools. After Stop every
//! program is gone (children included), every job is cancelled, nothing more
//! lands on the timeline and the plan says Stopped; Pause holds everything
//! at the next call and Resume carries on.

use super::brain::{AgentEvent, Brain, CodexBrain, Gate};
use super::crew::{self, CrewBrain, Halt, Role, Status};
use super::live::{Live, ToolCall};
use super::{Content, Session, procs};
use crate::doc::Doc;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Rig {
    gate: Arc<Gate>,
    live: Live,
    doc: Doc,
    calls: Receiver<ToolCall>,
    late: Sender<ToolCall>,
    events: Receiver<AgentEvent>,
    turn: Option<std::thread::JoinHandle<Result<(), String>>>,
    tool_starts: usize,
    plan: Option<crew::Plan>,
}

impl Rig {
    fn start(cast: crew::Casting) -> Rig {
        let (calls_tx, calls) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let gate = Arc::new(Gate::new(calls_tx.clone(), events_tx, || {}));
        let mut live = Live::new(|| {});
        gate.set_owner(live.owner());
        live.begin_turn("AI: crew");
        let g = gate.clone();
        let turn = std::thread::spawn(move || {
            g.begin();
            let mut brain = CrewBrain::new(cast);
            let r = brain.turn("make a short film", &g);
            g.end();
            r
        });
        Rig { gate, live, doc: Session::default().doc, calls, late: calls_tx, events, turn: Some(turn), tool_starts: 0, plan: None }
    }

    fn pump(&mut self) {
        self.live.tick(&mut self.doc, &self.calls);
        for e in self.events.try_iter() {
            match e {
                AgentEvent::ToolStart { .. } => self.tool_starts += 1,
                AgentEvent::Plan(p) => self.plan = Some(p),
                AgentEvent::Ask { answer, .. } => {
                    let _ = answer.send(super::brain::Answer::Apply);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    fn pump_for(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            self.pump();
        }
    }

    fn until(&mut self, what: &str, secs: u64, f: impl Fn(&Rig) -> bool) {
        let end = Instant::now() + Duration::from_secs(secs);
        while !f(self) {
            assert!(Instant::now() < end, "timed out waiting for: {what}");
            self.pump();
        }
    }

    fn finished(&self) -> bool {
        self.turn.as_ref().is_some_and(|t| t.is_finished())
    }

    fn join(&mut self) -> Result<(), String> {
        self.turn.take().expect("turn").join().unwrap_or_else(|_| Err("crashed".into()))
    }

    /// The job list as the AI's `job` tool reports it.
    fn job_states(&mut self) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        self.live.push_agent(ToolCall { tool: "job".into(), args: json!({}), reply: tx, epoch: 0 });
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.live.tick(&mut self.doc, &self.calls);
            if let Ok(r) = rx.try_recv() {
                let text = r.unwrap().into_iter().find_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).unwrap();
                let v: Value = serde_json::from_str(&text).unwrap();
                return v["jobs"].as_array().unwrap().iter().map(|j| j["state"].as_str().unwrap().to_owned()).collect();
            }
            assert!(Instant::now() < end, "the job list never answered");
        }
    }
}

/// A call from a crew member's brain.
fn call(gate: &Gate, role: Option<Role>, tool: &str, args: Value) -> Result<(), String> {
    gate.call_as(role, tool, &args).map(|_| ())
}

fn card(i: usize) -> Value {
    json!({"ops": [{"op": "card", "colour": "#336699", "at": i as f64, "duration": 1}]})
}

/// Plans the given roles (no dependencies), then approves at review.
struct Director {
    roles: Vec<&'static str>,
    turns: u32,
}

impl Brain for Director {
    fn forget(&mut self) {}
    fn turn(&mut self, _: &str, gate: &Gate) -> Result<(), String> {
        self.turns += 1;
        let r = Some(Role::Director);
        if self.turns == 1 {
            call(gate, r, "capabilities", json!({"query": "cards"}))?;
            let steps: Vec<Value> = self.roles.iter().map(|role| json!({"role": role, "title": format!("{role} work"), "task": "do your part"})).collect();
            call(gate, r, "plan", json!({"goal": "a short film", "steps": steps}))?;
        } else {
            call(gate, r, "plan", json!({"approve": true, "summary": "fine"}))?;
        }
        Ok(())
    }
}

/// Adds `n` cards, each a slow edit, one after the other.
struct Cards {
    role: Role,
    n: usize,
    base: usize,
}

impl Brain for Cards {
    fn forget(&mut self) {}
    fn turn(&mut self, _: &str, gate: &Gate) -> Result<(), String> {
        for i in 0..self.n {
            let at = self.base + i;
            let args = json!({"ms": 100, "limit_ms": 60_000, "edit": [{"op": "card", "colour": "#336699", "at": at as f64, "duration": 1}]});
            call(gate, Some(self.role), "__slow", args)?;
        }
        Ok(())
    }
}

/// Keeps adding cards, and starts a long background tool, until stopped.
struct Endless {
    role: Role,
}

impl Brain for Endless {
    fn forget(&mut self) {}
    fn turn(&mut self, _: &str, gate: &Gate) -> Result<(), String> {
        for i in 0.. {
            if call(gate, Some(self.role), "edit", card(i)).is_err() {
                return Ok(());
            }
            if i == 1 {
                let args = json!({"ms": 60_000, "limit_ms": 120_000, "edit": [{"op": "card", "colour": "#ff0000", "at": 90.0, "duration": 1}]});
                let _ = call(gate, Some(self.role), "__slow", args);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

/// A stand-in for `codex`: talks all the time, and has children of its own
/// (one of them in a subshell, one plain), whose pids it writes to `pids`.
fn fake_codex(dir: &Path, pids: &Path) -> PathBuf {
    let exe = dir.join("codex");
    let script = format!(
        "#!/bin/sh\necho $$ >> '{p}'\nsleep 601 &\necho $! >> '{p}'\n( sleep 602 & echo $! >> '{p}'; wait ) &\necho $! >> '{p}'\necho '{{\"type\":\"thread.started\",\"thread_id\":\"t1\"}}'\nwhile :; do echo '{{\"type\":\"turn.started\"}}'; sleep 0.05; done\n",
        p = pids.display()
    );
    std::fs::write(&exe, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    exe
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-crewstop-{name}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_pids(f: &Path) -> Vec<u32> {
    std::fs::read_to_string(f).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn codex(exe: &Path, dir: &Path, role: Role) -> Box<dyn Brain> {
    let mut b = CodexBrain::new(PathBuf::from("/nonexistent/studio.sock"), dir.to_owned(), None);
    b.role = Some(role);
    b.exe = Some(exe.to_owned());
    Box::new(b)
}

/// The command lines of every process on the machine.
fn process_list() -> String {
    let out = std::process::Command::new("ps").args(["-A", "-o", "command="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn stop_ends_every_program_job_and_edit_of_a_crew() {
    let dir = temp_dir("stop");
    let pids_file = dir.join("pids.txt");
    let exe = fake_codex(&dir, &pids_file);
    let (e2, d2) = (exe.clone(), dir.clone());
    let cast: crew::Casting = Box::new(move |role| {
        Ok(match role {
            Some(Role::Director) => Box::new(Director { roles: vec!["editor", "sound", "animator"], turns: 0 }) as Box<dyn Brain>,
            Some(r @ (Role::Editor | Role::Sound)) => codex(&e2, &d2, r),
            Some(r) => Box::new(Endless { role: r }),
            None => return Err("no solo".into()),
        })
    });
    let mut rig = Rig::start(cast);
    // Three members working: two Codex programs (each with children) and a
    // brain with a long background job and edits landing.
    rig.until("the crew at work", 60, |r| read_pids(&pids_file).len() >= 8 && r.doc.project.clips.len() >= 2);
    rig.until("a background job", 30, |r| r.live.has_running_jobs());
    let plan = rig.gate.board.snapshot();
    assert_eq!(plan.steps.iter().filter(|s| s.status == Status::Running).count(), 3, "{}", plan.text());
    assert!(plan.minutes_left().is_some(), "time is shown while it runs");
    let pids = read_pids(&pids_file);
    assert!(pids.iter().all(|p| procs::alive(*p)), "every program runs before Stop: {pids:?}");
    assert!(process_list().contains(dir.to_str().unwrap()), "the fake codex programs are listed");
    assert_eq!(rig.job_states(), vec!["running"]);

    // Stop.
    let epoch = rig.gate.halt();
    rig.live.cancel_at(&mut rig.doc, epoch);
    let at_stop = rig.doc.project.clips.len();

    // A call from before Stop that arrives late is refused.
    let (tx, rx) = mpsc::channel();
    rig.late.send(ToolCall { tool: "edit".into(), args: card(77), reply: tx, epoch: epoch - 1 }).unwrap();
    rig.pump();
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err(), "a late call is refused");

    // Every program, and every child of one, is gone.
    let end = Instant::now() + Duration::from_secs(10);
    while pids.iter().any(|p| procs::alive(*p)) {
        assert!(Instant::now() < end, "still running after Stop: {:?}", pids.iter().filter(|p| procs::alive(**p)).collect::<Vec<_>>());
        rig.pump();
    }
    assert!(!process_list().contains(dir.to_str().unwrap()), "no codex program of the crew is listed after Stop");
    assert_eq!(procs::count(rig.live.owner()), 0);
    // The crew winds down; the jobs are cancelled.
    rig.until("the request to end", 20, |r| r.finished());
    assert_eq!(rig.join(), Ok(()));
    assert_eq!(rig.job_states(), vec!["stopped"]);
    // Nothing lands afterwards.
    rig.pump_for(Duration::from_millis(800));
    assert_eq!(rig.doc.project.clips.len(), at_stop, "no edit landed after Stop");
    assert!(rig.doc.project.clips.iter().all(|c| c.start != 90 * crate::view::SEC), "the cancelled job's change never landed");
    // The plan says Stopped and nothing spins.
    let plan = rig.plan.clone().expect("a plan event");
    assert_eq!(plan.halted, Some(Halt::Stopped));
    assert!(plan.steps.iter().all(|s| s.status != Status::Running), "{}", plan.text());
    assert_eq!(plan.steps.iter().filter(|s| s.status == Status::Stopped).count(), 3);
    assert_eq!(plan.minutes_left(), None, "no time left on a stopped plan");
    assert_eq!(rig.gate.board.snapshot().halted, Some(Halt::Stopped));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pause_holds_at_the_next_call_and_resume_carries_on() {
    let cast: crew::Casting = Box::new(|role| {
        Ok(match role {
            Some(Role::Director) => Box::new(Director { roles: vec!["editor", "animator"], turns: 0 }) as Box<dyn Brain>,
            Some(Role::Editor) => Box::new(Cards { role: Role::Editor, n: 8, base: 0 }),
            Some(r) => Box::new(Cards { role: r, n: 8, base: 20 }),
            None => return Err("no solo".into()),
        })
    });
    let mut rig = Rig::start(cast);
    rig.until("the first edit", 60, |r| !r.doc.project.clips.is_empty());
    rig.gate.pause(true);
    rig.live.set_paused(true);
    // The call under way finishes; nothing new starts.
    rig.pump_for(Duration::from_millis(600));
    let (clips, starts) = (rig.doc.project.clips.len(), rig.tool_starts);
    assert!(clips >= 1 && clips < 16, "paused part way: {clips}");
    rig.pump_for(Duration::from_millis(900));
    assert_eq!((rig.doc.project.clips.len(), rig.tool_starts), (clips, starts), "nothing started or landed while paused");
    assert!(!rig.finished());
    let plan = rig.plan.clone().expect("a plan event");
    assert_eq!(plan.halted, Some(Halt::Paused), "the card shows Paused at once");
    assert_eq!(plan.minutes_left(), None, "no time left while paused");
    assert!(plan.steps.iter().all(|s| s.status == Status::Running), "the steps wait where they are: {}", plan.text());

    // Resume: the same request goes on to the end.
    rig.gate.pause(false);
    rig.live.set_paused(false);
    rig.until("the request to end", 90, |r| r.finished());
    assert_eq!(rig.join(), Ok(()));
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(rig.doc.project.clips.len(), 16);
    let plan = rig.plan.clone().unwrap();
    assert_eq!(plan.halted, None);
    assert!(plan.steps.iter().all(|s| s.status == Status::Done), "{}", plan.text());
}
