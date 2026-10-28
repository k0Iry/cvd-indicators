//! Minimal synchronous client for the real `cvd-indicator-runner`, used by the
//! integration tests. It speaks the frozen v1 frame protocol through the SDK's
//! own `protocol` module (no duplicated schema) and always spawns the runner
//! the way the host does: empty environment, cwd `/`, stdout = frames only.

use cvd_indicator_sdk::protocol::{read_frame, write_frame, FrameHeader, FrameKind};
use cvd_indicator_sdk::{Descriptor, InputBar, Mode, Output, PluginError, Scope};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub const MAX_PAYLOAD: usize = 32 << 20;

pub struct Runner {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    next_id: u64,
    pub descriptor: Descriptor,
    pub descriptor_json: Vec<u8>,
}

#[derive(Debug)]
pub enum Reply {
    Ok(Output),
    Err(PluginError),
}

impl Reply {
    pub fn unwrap(self) -> Output {
        match self {
            Reply::Ok(o) => o,
            Reply::Err(e) => panic!("plugin error: {e}"),
        }
    }
    pub fn unwrap_err(self) -> PluginError {
        match self {
            Reply::Err(e) => e,
            Reply::Ok(o) => panic!("expected error, got {o:?}"),
        }
    }
}

impl Runner {
    pub fn spawn(runner: &Path, library: &Path) -> Result<Self, String> {
        let mut child = Command::new(runner)
            .arg("--library")
            .arg(library)
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", runner.display()))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut me = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            descriptor_json: vec![],
            descriptor: empty_descriptor(),
        };
        let hello = serde_json::to_vec(
            &serde_json::json!({"protocol": 1, "abi": cvd_indicator_sdk::abi::ABI_VERSION}),
        )
        .unwrap();
        let (kind, body) = me.round_trip(FrameKind::Hello, 0, 0, &[], &hello)?;
        if kind != FrameKind::Ready {
            return Err(format!(
                "handshake rejected: {}",
                String::from_utf8_lossy(&body)
            ));
        }
        me.descriptor = serde_json::from_slice(&body).map_err(|e| format!("descriptor: {e}"))?;
        me.descriptor.validate().map_err(|e| e.to_string())?;
        me.descriptor_json = body;
        Ok(me)
    }

    fn round_trip(
        &mut self,
        kind: FrameKind,
        session: u64,
        generation: u64,
        params: &[u8],
        body: &[u8],
    ) -> Result<(FrameKind, Vec<u8>), String> {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(
            &mut self.stdin,
            FrameHeader::new(kind, id, session, generation),
            params,
            body,
        )
        .map_err(|e| e.to_string())?;
        let frame = read_frame(&mut self.stdout, MAX_PAYLOAD)
            .map_err(|e| e.to_string())?
            .ok_or("runner closed stdout")?;
        let h = frame.header;
        if h.request_id != id || h.session != session || h.generation != generation {
            return Err(format!("identity mismatch: {h:?}"));
        }
        Ok((h.kind, frame.body().to_vec()))
    }

    fn reply(
        &mut self,
        kind: FrameKind,
        session: u64,
        generation: u64,
        params: &[u8],
        body: &[u8],
    ) -> Reply {
        match self
            .round_trip(kind, session, generation, params, body)
            .expect("runner protocol")
        {
            (FrameKind::Result, b) => Reply::Ok(serde_json::from_slice(&b).expect("output json")),
            (FrameKind::Error, b) => Reply::Err(serde_json::from_slice(&b).expect("error json")),
            (k, _) => panic!("unexpected frame {k:?}"),
        }
    }

    pub fn history(
        &mut self,
        params: &str,
        scope: &Scope,
        output_from: usize,
        bars: &[InputBar],
    ) -> Reply {
        let body = serde_json::to_vec(&cvd_indicator_sdk::HistoryRequest {
            scope: scope.clone(),
            revision: 1,
            output_from,
            bars: bars.to_vec(),
        })
        .unwrap();
        self.reply(FrameKind::History, 0, 1, params.as_bytes(), &body)
    }

    pub fn live_open(
        &mut self,
        session: u64,
        generation: u64,
        params: &str,
        scope: &Scope,
        bars: &[InputBar],
    ) -> Reply {
        let body = serde_json::to_vec(&cvd_indicator_sdk::LiveInit {
            scope: scope.clone(),
            revision: generation,
            bars: bars.to_vec(),
        })
        .unwrap();
        self.reply(
            FrameKind::LiveOpen,
            session,
            generation,
            params.as_bytes(),
            &body,
        )
    }

    pub fn live_update(
        &mut self,
        session: u64,
        generation: u64,
        seq: u64,
        ops: Vec<cvd_indicator_sdk::LiveOp>,
    ) -> Reply {
        let body = serde_json::to_vec(&cvd_indicator_sdk::LiveUpdate {
            revision: generation,
            seq,
            ops,
        })
        .unwrap();
        self.reply(FrameKind::LiveUpdate, session, generation, &[], &body)
    }

    pub fn shutdown(mut self) -> std::process::ExitStatus {
        let _ = write_frame(
            &mut self.stdin,
            FrameHeader::new(FrameKind::Shutdown, self.next_id, 0, 0),
            &[],
            &[],
        );
        self.child.wait().expect("runner exit")
    }
}

fn empty_descriptor() -> Descriptor {
    Descriptor {
        id: String::new(),
        version: String::new(),
        name: String::new(),
        description: String::new(),
        abi: 0,
        context: Default::default(),
        params: vec![],
        inputs: vec![],
        outputs: vec![],
        live: false,
    }
}

pub fn scope(mode: Mode) -> Scope {
    Scope {
        exchange: "binance".into(),
        market: "spot".into(),
        symbol: "BTC".into(),
        tf: "1m".into(),
        mode,
    }
}

/// Synthetic loopback bar. `cvd`/`ac` are deliberately NOT the running sum of
/// the deltas, so a projection can be told apart from a recomputation.
pub fn bar(i: i64) -> InputBar {
    let f = i as f64;
    let price = 30_000.0 + 50.0 * (f / 7.0).sin();
    let (buy, sell) = (5_000.0 + 1_000.0 * (f / 3.0).sin(), 5_000.0);
    InputBar {
        time: 1_700_000_040 + 60 * i,
        po: price - 1.0,
        ph: price + 5.0,
        pl: price - 5.0,
        price_close: price,
        apo: price + 9.0,
        aph: price + 15.0,
        apl: price + 5.0,
        apc: price + 10.0,
        cvd_usdt: 1_000_000.0 + 37.0 * f,
        ac: -2_000_000.0 - 11.0 * f,
        delta_usdt: buy - sell,
        ad: 2.0 * (buy - sell),
        bvb: buy / price,
        abvb: 2.0 * buy / price,
        svb: sell / price,
        asvb: 2.0 * sell / price,
        buy_volume_usdt: buy,
        agb: 2.0 * buy,
        sell_volume_usdt: sell,
        ags: 2.0 * sell,
        trades: 10 + i as u64,
    }
}

/// Locate (building if needed) the cdylib for `package` in this workspace.
pub fn library(package: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let status = Command::new(env!("CARGO"))
        .args(["build", "--offline", "-p", package])
        .current_dir(&root)
        .status()
        .expect("cargo build");
    assert!(status.success(), "building {package} failed");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let path = target
        .join("debug")
        .join(format!("lib{}.so", package.replace('-', "_")));
    assert!(path.is_file(), "missing {}", path.display());
    path
}

/// The public runner binary under test. Required: there is no in-process
/// fallback, because loading through the real runner is the point.
pub fn runner_binary() -> PathBuf {
    let path = std::env::var_os("CVD_INDICATOR_RUNNER").map(PathBuf::from).unwrap_or_else(|| {
        panic!("set CVD_INDICATOR_RUNNER to a built cvd-core `cvd-indicator-runner` (see README: Testing)")
    });
    assert!(
        path.is_file(),
        "CVD_INDICATOR_RUNNER={} is not a file",
        path.display()
    );
    path
}
