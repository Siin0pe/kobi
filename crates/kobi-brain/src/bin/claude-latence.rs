//! Mesure du temps jusqu'au premier token de Claude Code (KOB-30).
//!
//! ```text
//! cargo run --release -p kobi-brain --bin claude-latence -- \
//!     --mode froid|persistant [--config isolee|perso] [--mcp-config fichier.json] \
//!     [--n 20] [--model haiku] [--threads 3]
//! ```
//!
//! * `froid` : un process par question ; chrono lancé avant le spawn.
//! * `persistant` : un seul process ; une question de chauffe non comptée,
//!   puis chrono lancé à l'écriture de chaque question sur stdin.
//!
//! * `isolee` (défaut) : `--setting-sources project --strict-mcp-config`,
//!   seuls les serveurs de `--mcp-config` sont chargés, sans réglages, hooks
//!   ni plugins de l'utilisateur.
//! * `perso` : la config de l'utilisateur telle quelle, plus `--mcp-config`.
//!
//! Le premier token est le premier delta de texte reçu.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kobi_brain::{ClaudeCode, ClaudeOptions, Event};

const QUESTION: &str = "Réponds uniquement : ok";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Froid,
    Persistant,
}

struct Args {
    mode: Mode,
    perso: bool,
    mcp_config: Option<String>,
    n: usize,
    model: Option<String>,
    threads: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        mode: Mode::Persistant,
        perso: false,
        mcp_config: None,
        n: 20,
        model: None,
        threads: 3,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("valeur manquante pour {flag}"));
        match flag.as_str() {
            "--mode" => {
                args.mode = match value()?.as_str() {
                    "froid" => Mode::Froid,
                    "persistant" => Mode::Persistant,
                    m => return Err(format!("mode inconnu : {m}")),
                }
            }
            "--config" => {
                args.perso = match value()?.as_str() {
                    "isolee" => false,
                    "perso" => true,
                    c => return Err(format!("config inconnue : {c}")),
                }
            }
            "--mcp-config" => args.mcp_config = Some(value()?),
            "--n" => args.n = value()?.parse().map_err(|e| format!("--n : {e}"))?,
            "--model" => args.model = Some(value()?),
            "--threads" => {
                args.threads = value()?.parse().map_err(|e| format!("--threads : {e}"))?
            }
            f => return Err(format!("argument inconnu : {f}")),
        }
    }
    Ok(args)
}

/// Médiane et 90e centile (rang le plus proche), en secondes.
fn stats(samples: &[Duration]) -> (f64, f64) {
    let mut s: Vec<f64> = samples.iter().map(Duration::as_secs_f64).collect();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    let median = if n.is_multiple_of(2) {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    } else {
        s[n / 2]
    };
    let rank = (0.9 * n as f64).ceil() as usize;
    (median, s[rank.max(1) - 1])
}

/// Lit le tour jusqu'au `result` ; renvoie le délai du premier token depuis
/// `t0` et l'état des serveurs MCP annoncé par `init`.
async fn read_turn(claude: &mut ClaudeCode, t0: Instant) -> Result<(Duration, String), String> {
    let mut first = None;
    let mut mcp = String::new();
    loop {
        let event = claude
            .next_event()
            .await
            .ok_or("process claude terminé avant le résultat")?;
        if first.is_none() && event.text_delta().is_some() {
            first = Some(t0.elapsed());
        }
        match event {
            Event::System(s) if s.subtype == "init" => {
                let states: Vec<String> = s
                    .mcp_servers
                    .iter()
                    .map(|m| format!("{}:{}", m.name, m.status))
                    .collect();
                mcp = states.join(",");
            }
            Event::Result(r) if r.is_error => return Err(format!("tour en erreur : {r:?}")),
            Event::Result(_) => break,
            _ => {}
        }
    }
    Ok((first.ok_or("aucun delta de texte")?, mcp))
}

async fn measure(args: &Args, opts: &ClaudeOptions) -> Result<Vec<Duration>, String> {
    let mut samples = Vec::with_capacity(args.n);
    match args.mode {
        Mode::Froid => {
            for i in 0..args.n {
                let t0 = Instant::now();
                let mut claude = ClaudeCode::spawn(opts).map_err(|e| e.to_string())?;
                claude.send(QUESTION).await.map_err(|e| e.to_string())?;
                let (ttft, mcp) = read_turn(&mut claude, t0).await?;
                claude
                    .close(Duration::from_secs(10))
                    .await
                    .map_err(|e| e.to_string())?;
                eprintln!("{:>2} {:.3} [{mcp}]", i + 1, ttft.as_secs_f64());
                samples.push(ttft);
            }
        }
        Mode::Persistant => {
            let mut claude = ClaudeCode::spawn(opts).map_err(|e| e.to_string())?;
            claude.send(QUESTION).await.map_err(|e| e.to_string())?;
            let (warmup, mcp) = read_turn(&mut claude, Instant::now()).await?;
            eprintln!("chauffe {:.3} [{mcp}]", warmup.as_secs_f64());
            for i in 0..args.n {
                let t0 = Instant::now();
                claude.send(QUESTION).await.map_err(|e| e.to_string())?;
                let (ttft, _) = read_turn(&mut claude, t0).await?;
                eprintln!("{:>2} {:.3}", i + 1, ttft.as_secs_f64());
                samples.push(ttft);
            }
            claude
                .close(Duration::from_secs(10))
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(samples)
}

fn main() -> Result<(), String> {
    let args = parse_args()?;
    // Le client attend surtout le réseau ; on borne quand même ses threads
    // (règle de mesure : 3 threads au plus par défaut).
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(run(args))
}

async fn run(args: Args) -> Result<(), String> {
    let mut opts = ClaudeOptions::default();
    if !args.perso {
        opts.args
            .extend(["--setting-sources", "project", "--strict-mcp-config"].map(String::from));
    }
    if let Some(program) = std::env::var_os("KOBI_CLAUDE") {
        opts.program = PathBuf::from(program);
    }
    if let Some(cfg) = &args.mcp_config {
        opts.args.extend(["--mcp-config".into(), cfg.clone()]);
    }
    if let Some(model) = &args.model {
        opts.args.extend(["--model".into(), model.clone()]);
    }

    let samples = measure(&args, &opts).await?;
    let (median, p90) = stats(&samples);
    let all: Vec<String> = samples
        .iter()
        .map(|d| format!("{:.3}", d.as_secs_f64()))
        .collect();
    println!(
        "mode={:?} config={} mcp={} n={} mediane={median:.2}s p90={p90:.2}s mesures=[{}]",
        args.mode,
        if args.perso { "perso" } else { "isolee" },
        args.mcp_config.as_deref().unwrap_or("aucun"),
        samples.len(),
        all.join(" "),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: &[u64]) -> Vec<Duration> {
        v.iter().copied().map(Duration::from_millis).collect()
    }

    #[test]
    fn mediane_et_p90() {
        let (m, p) = stats(&ms(&[
            1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 10000,
        ]));
        assert_eq!(m, 5.5);
        assert_eq!(p, 9.0);
        let (m, p) = stats(&ms(&[3000, 1000, 2000]));
        assert_eq!(m, 2.0);
        assert_eq!(p, 3.0);
    }
}
