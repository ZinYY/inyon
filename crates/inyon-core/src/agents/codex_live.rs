//! Port of agents/codex-live.ts: Codex's reply as it is being written.
//!
//! The TUI is a client of Codex's shared app-server daemon, which other clients can join:
//! `thread/resume` on a running thread rejoins it, and from then on every delta, item and turn of
//! that thread arrives as a notification. A small node script on the machine does the WebSocket
//! framing and prints what arrives as JSON lines; [`CodexLiveState::apply`] reads those lines.

use std::collections::HashMap;

use indexmap::IndexMap;
use inyon_protocol::{LiveItem, LiveKind, SubagentStatus, Turn};
use serde::Serialize;
use serde_json::Value;

use crate::claude::transcript::string_of;
use crate::clock::now_ms;
use crate::features::setup::PATHS;
use crate::hosts::host::shq;
use crate::js::json::{self, value_string, Js};
use crate::js::{date, JsStr};
use crate::re;

/// The helper that follows a thread through the daemon's socket. It ends when its stdin closes,
/// which is how it's stopped.
pub const CODEX_LIVE_SCRIPT: &str = r##"
const net=require('node:net'),crypto=require('node:crypto'),os=require('node:os');
const thread=process.argv[1];
const home=process.env.CODEX_HOME||os.homedir()+'/.codex';
const out=(o)=>process.stdout.write(JSON.stringify(o)+'\n');
process.stdin.on('end',()=>process.exit(0));process.stdin.on('error',()=>process.exit(0));process.stdin.resume();
const sock=net.connect({path:home+'/app-server-control/app-server-control.sock'});
let up=false,buf=Buffer.alloc(0),parts=[],nextId=1;const pending=new Map();
function frame(op,payload){const mask=crypto.randomBytes(4),n=payload.length;let h;
if(n<126)h=Buffer.from([0x80|op,0x80|n]);else if(n<65536){h=Buffer.alloc(4);h[0]=0x80|op;h[1]=0xfe;h.writeUInt16BE(n,2);}else{h=Buffer.alloc(10);h[0]=0x80|op;h[1]=0xff;h.writeBigUInt64BE(BigInt(n),2);}
const m=Buffer.from(payload);for(let i=0;i<m.length;i++)m[i]^=mask[i&3];sock.write(Buffer.concat([h,mask,m]));}
const send=(msg)=>frame(1,Buffer.from(JSON.stringify(msg)));
const request=(method,params)=>new Promise((resolve)=>{const id=nextId++;pending.set(id,resolve);send({jsonrpc:'2.0',id,method,params});});
function frames(){while(buf.length>=2){const fin=(buf[0]&0x80)!==0,op=buf[0]&0x0f,masked=(buf[1]&0x80)!==0;let n=buf[1]&0x7f,off=2;
if(n===126){if(buf.length<4)return;n=buf.readUInt16BE(2);off=4;}else if(n===127){if(buf.length<10)return;n=Number(buf.readBigUInt64BE(2));off=10;}
if(masked)off+=4;if(buf.length<off+n)return;const payload=buf.subarray(off,off+n);buf=buf.subarray(off+n);
if(op===9)frame(10,payload);else if(op===8){out({e:'closed'});process.exit(1);}
else if(op===1||op===0){parts.push(Buffer.from(payload));if(fin){const text=Buffer.concat(parts).toString();parts=[];onText(text);}}}}
const DROP=new Set(['turn/moderationMetadata','thread/readState/changed','account/updated','remoteControl/status/changed','rawResponse/completed','rawResponseItem/completed','model/verification','model/safetyBuffering/updated']);
const children=new Set();
function onText(text){let m;try{m=JSON.parse(text)}catch{return}
if(m.id!==undefined&&!m.method){const r=pending.get(m.id);if(r){pending.delete(m.id);r(m);}return;}
const p=m.params||{};
if(m.method==='thread/started'){if(p.thread&&p.thread.parentThreadId===thread)children.add(p.thread.id);else if(!p.thread||p.thread.id!==thread)return;}
else if(p.threadId!==thread&&!children.has(p.threadId))return;
if(DROP.has(m.method))return;
// a request the daemon makes of its clients (an approval, a question): reported, never answered here, the TUI answers it
if(m.id!==undefined)out({m:m.method,p:p,req:true});else out({m:m.method,p:p});}
sock.on('connect',()=>{const key=crypto.randomBytes(16).toString('base64');sock.write('GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: '+key+'\r\n\r\n');});
sock.on('data',(d)=>{buf=Buffer.concat([buf,d]);if(!up){const end=buf.indexOf('\r\n\r\n');if(end<0)return;const head=buf.subarray(0,end).toString();buf=buf.subarray(end+4);
if(!/ 101 /.test(head)){out({e:'refused',head:head.split('\n')[0]});process.exit(1);}up=true;ready();}frames();});
sock.on('error',(e)=>{out({e:'error',message:e.message});process.exit(1);});
sock.on('close',()=>{out({e:'closed'});process.exit(1);});
async function ready(){await request('initialize',{clientInfo:{name:'inyon',title:'Inyon',version:'0'},capabilities:{experimentalApi:true,requestAttestation:false}});send({jsonrpc:'2.0',method:'initialized',params:{}});
// the thread has no rollout until its first message, and can't be joined before then
for(;;){const r=await request('thread/resume',{threadId:thread,excludeTurns:true});
if(!r.error){const t=r.result||{};out({e:'resumed',model:t.model,effort:t.reasoningEffort,approval:t.approvalPolicy,tier:t.serviceTier,status:t.thread&&t.thread.status});return;}
out({e:'waiting',error:r.error.message});await new Promise((f)=>setTimeout(f,3000));}}
"##;

/// The command that follows thread `thread_id` on the machine; stop it by closing its stdin.
pub fn codex_live_command(thread_id: &str) -> String {
    format!("{PATHS}; node -e {} {}", shq(CODEX_LIVE_SCRIPT), shq(thread_id))
}

/// The thread a rollout file belongs to (`rollout-<time>-<thread>[_<segment>].jsonl`).
pub fn codex_thread_id(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("");
    let name = re!(r"\.jsonl$").replace(name, "");
    let name = re!(r"^rollout-[\dT-]+-").replace(&name, "");
    re!(r"_[0-9a-f-]{36}$").replace(&name, "").into_owned()
}

/// What one line of the helper changed, so the watcher knows what to publish.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LiveChange {
    /// the live text changed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<bool>,
    /// a turn started or ended
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<bool>,
    /// the thread's status changed (working, idle, waiting for an approval or an answer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<bool>,
    /// token usage or limits were reported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<bool>,
    /// Codex (re)named the thread
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// the queue of follow-ups changed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<bool>,
    /// the daemon asked its clients something (an approval, a question) or one was answered
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<bool>,
    /// a sub-agent started or finished
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgent {
    pub thread_id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub status: SubagentStatus,
    pub started_at: String,
}

/// Settings reported when the thread was joined.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LiveSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
}

const OUTPUT_KEEP: isize = 4000;

fn yes() -> Option<bool> {
    Some(true)
}

/// `ms` as an ISO time; now when it isn't a number or is out of a Date's range
fn iso(ms: Option<f64>) -> String {
    ms.and_then(date::to_iso).or_else(|| date::to_iso(now_ms() as f64)).unwrap_or_default()
}

fn flags(v: Option<&Value>) -> Vec<String> {
    v.arr().map(|a| a.iter().map(value_string).collect()).unwrap_or_default()
}

/// What the live helper has said so far: the items being written (by id, oldest first), the turn
/// under way and the thread's status.
#[derive(Debug, Clone, Default)]
pub struct CodexLiveState {
    pub items: IndexMap<String, LiveItem>,
    /// items Codex has finished, with when, until their record shows up in the rollout
    pub done: IndexMap<String, f64>,
    pub turn: Option<Turn>,
    /// active, idle, notLoaded or systemError
    pub status: Option<String>,
    pub flags: Vec<String>,
    pub agents: IndexMap<String, LiveAgent>,
    pub settings: Option<LiveSettings>,
    summary_index: HashMap<String, Value>,
}

impl CodexLiveState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one line of the helper's output and says what changed.
    pub fn apply(&mut self, line: &str) -> LiveChange {
        let Some(msg) = json::parse(line) else { return LiveChange::default() };
        let msg = Some(&msg);
        if msg.at("e").str() == Some("resumed") {
            self.settings = Some(LiveSettings { model: string_of(msg.at("model")), effort: string_of(msg.at("effort")), tier: string_of(msg.at("tier")) });
            let t = msg.at("status").at("type");
            if t.truthy() {
                self.status = t.map(value_string);
                self.flags = flags(msg.at("status").at("activeFlags"));
            }
            return LiveChange { status: yes(), ..Default::default() };
        }
        let Some(method) = msg.at("m").str() else { return LiveChange::default() };
        let empty = Value::Object(Default::default());
        let p = Some(msg.at("p").present().unwrap_or(&empty));
        if msg.at("req").truthy() {
            return LiveChange { ask: yes(), ..Default::default() };
        }
        let item_id = string_of(p.at("itemId")).filter(|_| p.at("itemId").truthy());
        let turn_id = string_of(p.at("turnId"));
        let text = p.at("delta").str().map(str::to_owned);
        match method {
            "item/agentMessage/delta" => self.delta(item_id, LiveKind::Text, text, turn_id),
            "item/plan/delta" => self.delta(item_id, LiveKind::Plan, text, turn_id),
            "item/reasoning/summaryTextDelta" => {
                // each summary is a paragraph of its own
                let key = p.at("itemId").to_js_string();
                let index = p.at("summaryIndex").cloned();
                let prev = self.summary_index.get(&key);
                // `p.summaryIndex !== prev`
                let same = matches!((index.as_ref(), prev), (Some(a), Some(b)) if crate::js::json::strict_equals(a, b));
                let gap = prev.is_some() && !same && self.items.contains_key(&key);
                match index {
                    Some(i) => self.summary_index.insert(key, i),
                    None => self.summary_index.remove(&key),
                };
                let text = format!("{}{}", if gap { "\n\n" } else { "" }, p.at("delta").to_js_string());
                self.delta(item_id, LiveKind::Thinking, Some(text), turn_id)
            }
            "item/commandExecution/outputDelta" => self.delta(item_id, LiveKind::Output, text, turn_id),
            "item/started" => {
                let item = p.at("item");
                if item.at("type").str() == Some("subAgentActivity") && item.at("kind").str() == Some("started") {
                    return self.agent_started(item, p.at("startedAtMs").num());
                }
                LiveChange::default()
            }
            "item/completed" => {
                let item = p.at("item");
                if item.at("type").str() == Some("subAgentActivity") {
                    return self.agent_ended(item);
                }
                let id = item.at("id").to_js_string();
                let Some(live) = self.items.get_mut(&id) else { return LiveChange::default() };
                // the finished text, in case a delta went missing
                let kind = item.at("type").str();
                let summary = item.at("summary").arr().filter(|a| !a.is_empty());
                if let (Some("agentMessage"), Some(t)) = (kind, item.at("text").nonempty()) {
                    live.text = t.to_owned();
                } else if let (Some("reasoning"), Some(s)) = (kind, summary) {
                    live.text = s.iter().map(|x| if x.is_null() { String::new() } else { value_string(x) }).collect::<Vec<_>>().join("\n\n");
                } else if let (Some("plan"), Some(t)) = (kind, item.at("text").nonempty()) {
                    live.text = t.to_owned();
                }
                self.done.insert(id, p.at("completedAtMs").num().unwrap_or_else(|| now_ms() as f64));
                LiveChange { live: yes(), ..Default::default() }
            }
            "turn/started" => {
                let started = p.at("turn").at("startedAt").num().unwrap_or(now_ms() as f64 / 1000.0);
                self.turn = Some(Turn { id: string_of(p.at("turn").at("id")), started_at: inyon_protocol::Num(started * 1000.0) });
                LiveChange { turn: yes(), ..Default::default() }
            }
            "turn/completed" => {
                self.turn = None;
                // whatever was still being written is over; with no turn id there's no telling what
                let turn = string_of(p.at("turn").at("id"));
                let now = now_ms() as f64;
                for (id, it) in &self.items {
                    if turn.is_some() && it.turn_id == turn && !self.done.contains_key(id) {
                        self.done.insert(id.clone(), now);
                    }
                }
                LiveChange { turn: yes(), live: yes(), ..Default::default() }
            }
            "thread/status/changed" => {
                let kind = p.at("status").at("type").str();
                if let Some(a) = p.at("threadId").filter(|_| p.at("threadId").truthy()).map(value_string).and_then(|t| self.agents.get_mut(&t)) {
                    if kind == Some("idle") && a.status == SubagentStatus::Running {
                        a.status = SubagentStatus::Done;
                    }
                    if kind == Some("systemError") {
                        a.status = SubagentStatus::Error;
                    }
                    return LiveChange { agents: yes(), ..Default::default() };
                }
                self.status = string_of(p.at("status").at("type"));
                self.flags = flags(p.at("status").at("activeFlags"));
                LiveChange { status: yes(), ..Default::default() }
            }
            "thread/started" => {
                let t = p.at("thread");
                let id = t.at("id").to_js_string();
                if t.at("parentThreadId").truthy() && !self.agents.contains_key(&id) {
                    let description = t.at("preview").filter(|_| t.at("preview").truthy()).map_or_else(|| "Sub-agent".into(), value_string);
                    self.agents.insert(
                        id.clone(),
                        LiveAgent {
                            thread_id: id,
                            description,
                            model: string_of(t.at("model")),
                            status: SubagentStatus::Running,
                            started_at: iso(t.at("createdAt").num().map(|s| s * 1000.0)),
                        },
                    );
                    return LiveChange { agents: yes(), ..Default::default() };
                }
                LiveChange::default()
            }
            "thread/tokenUsage/updated" | "account/rateLimits/updated" => LiveChange { usage: yes(), ..Default::default() },
            "thread/name/updated" => LiveChange { name: Some(string_of(p.at("threadName")).unwrap_or_default()), ..Default::default() },
            "thread/queue/changed" => LiveChange { queue: yes(), ..Default::default() },
            "serverRequest/resolved" => LiveChange { ask: yes(), ..Default::default() },
            _ => LiveChange::default(),
        }
    }

    fn delta(&mut self, id: Option<String>, kind: LiveKind, text: Option<String>, turn_id: Option<String>) -> LiveChange {
        let (Some(id), Some(text)) = (id, text) else { return LiveChange::default() };
        let it = self.items.entry(id.clone()).or_insert_with(|| LiveItem { id: id.clone(), kind, text: String::new(), turn_id });
        it.text.push_str(&text);
        // a command's output can run on for pages; the end is what's being watched
        if kind == LiveKind::Output && it.text.len16() > OUTPUT_KEEP as usize {
            it.text = it.text.slice16(-OUTPUT_KEEP, None).to_owned();
        }
        self.done.shift_remove(&id);
        LiveChange { live: yes(), ..Default::default() }
    }

    fn agent_started(&mut self, item: Option<&Value>, at: Option<f64>) -> LiveChange {
        if !item.at("agentThreadId").truthy() {
            return LiveChange::default();
        }
        let id = item.at("agentThreadId").to_js_string();
        let cur = self.agents.get(&id);
        let agent = LiveAgent {
            thread_id: id.clone(),
            description: cur.map(|c| c.description.clone()).or_else(|| string_of(item.at("agentPath"))).unwrap_or_else(|| "Sub-agent".into()),
            model: string_of(item.at("model")).or_else(|| cur.and_then(|c| c.model.clone())),
            status: SubagentStatus::Running,
            started_at: cur.map(|c| c.started_at.clone()).unwrap_or_else(|| iso(at)),
        };
        self.agents.insert(id, agent);
        LiveChange { agents: yes(), ..Default::default() }
    }

    fn agent_ended(&mut self, item: Option<&Value>) -> LiveChange {
        let Some(a) = self.agents.get_mut(&item.at("agentThreadId").to_js_string()) else { return LiveChange::default() };
        match item.at("kind").str() {
            Some("completed") => a.status = SubagentStatus::Done,
            Some("interrupted") => a.status = SubagentStatus::Error,
            _ => {}
        }
        LiveChange { agents: yes(), ..Default::default() }
    }

    /// A finished item's record arrived in the rollout (or it never will): drops the live copy.
    /// Returns true when something was dropped.
    pub fn settle(&mut self, ids: &[Option<&str>]) -> bool {
        let mut dropped = false;
        for id in ids.iter().flatten().filter(|id| !id.is_empty()) {
            if self.items.shift_remove(*id).is_some() {
                dropped = true;
            }
            self.done.shift_remove(*id);
        }
        dropped
    }

    /// Drops finished items whose record hasn't shown up within `ms`; true when any went.
    pub fn expire(&mut self, ms: f64, now: f64) -> bool {
        let old: Vec<String> = self.done.iter().filter(|(_, at)| now - **at >= ms).map(|(id, _)| id.clone()).collect();
        for id in &old {
            self.items.shift_remove(id);
            self.done.shift_remove(id);
        }
        !old.is_empty()
    }

    /// The items to show, oldest first.
    pub fn list(&self) -> Vec<LiveItem> {
        self.items.values().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.done.clear();
        self.turn = None;
        self.summary_index.clear();
    }
}

#[cfg(test)]
#[path = "codex_live_tests.rs"]
mod tests;
