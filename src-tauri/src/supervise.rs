//! `codeg-server --supervise` — a minimal process supervisor that owns the
//! lifecycle of the real worker so an in-place upgrade can swap the binary
//! and have the *new* version relaunched deterministically.
//!
//! In Docker this process is PID 1, so it also has to behave like an init:
//! forward `SIGTERM`/`SIGINT` to the worker for graceful container stop, and
//! reap reparented orphan children so they don't pile up as zombies.
//!
//! Relaunch contract: the worker performs the file swap and then exits with
//! [`crate::update::runtime::EXIT_RESTART`]. We wait `CODEG_RESTART_DELAY_MS`
//! and respawn from the (now replaced) executable path.
//!
//! Failed-upgrade self-heal: a worker we just relaunched *because of* an
//! upgrade is on probation. If it exits abnormally within
//! `CODEG_UPGRADE_TRIAL_SECS` (the new binary can't even boot), we restore
//! the previous bundle from its `.bak` artifacts and relaunch that — once.
//! This is the one recovery path that does **not** depend on the (now dead)
//! HTTP rollback endpoint. Any other exit is propagated as-is — a fatal
//! config error or a crash after the trial window should stop the process
//! (and, under a container restart policy, fall back to the image) rather
//! than hot-loop.

use crate::update::runtime;

/// Build the worker argv: our own args minus the `--supervise` flag.
fn worker_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| a != "--supervise")
        .collect()
}

/// Restore the previous bundle from its `.bak` artifacts after a freshly
/// upgraded worker failed to stay up. Returns `true` if a rollback was
/// performed (caller should relaunch the restored, previous version).
/// Best-effort: a missing `.bak` (nothing to revert to) returns `false` and
/// the caller propagates the original failure.
fn attempt_rollback(reason: &str) -> bool {
    eprintln!("[supervise] {reason}; rolling back to the previous version");
    match crate::update::install::rollback() {
        Ok(()) => {
            eprintln!("[supervise] rollback complete; relaunching previous version");
            true
        }
        Err(e) => {
            eprintln!("[supervise] rollback failed: {e}; propagating original exit");
            false
        }
    }
}

#[cfg(unix)]
pub fn run() -> ! {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static WORKER_PID: AtomicI32 = AtomicI32::new(0);
    static TERMINATING: AtomicBool = AtomicBool::new(false);

    extern "C" fn forward_signal(sig: libc::c_int) {
        TERMINATING.store(true, Ordering::SeqCst);
        let pid = WORKER_PID.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: `kill` is async-signal-safe.
            unsafe {
                libc::kill(pid, sig);
            }
        }
    }

    // SAFETY: installing handlers before spawning any child. `signal` is fine
    // for our forward-and-flag use; we deliberately keep the handler trivial.
    unsafe {
        libc::signal(libc::SIGTERM, forward_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, forward_signal as *const () as libc::sighandler_t);
    }

    let exe = runtime::self_exe();
    let args = worker_args();
    let delay = runtime::restart_delay_ms();
    let trial = std::time::Duration::from_secs(runtime::upgrade_trial_secs());

    let spawn_worker = || -> std::process::Child {
        std::process::Command::new(&exe)
            .args(&args)
            .env(runtime::ENV_SUPERVISED, "1")
            .env(runtime::ENV_RESTART_DELAY_MS, delay.to_string())
            .spawn()
            .unwrap_or_else(|e| {
                eprintln!("[supervise][FATAL] failed to spawn worker: {e}");
                std::process::exit(1);
            })
    };

    // Spawn + publish the PID, then close the spawn→store race: if a stop
    // signal landed in that window the handler couldn't forward it (the PID
    // was still 0), so forward it here.
    let spawn_and_track = || -> std::process::Child {
        let child = spawn_worker();
        let pid = child.id() as i32;
        WORKER_PID.store(pid, Ordering::SeqCst);
        if TERMINATING.load(Ordering::SeqCst) {
            // SAFETY: `kill` is async-signal-safe; `pid` is our just-spawned child.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        child
    };

    // A worker is "on trial" when this launch is the trial of a freshly
    // swapped version — i.e. a staged-upgrade marker was present at launch. A
    // plain restart (no pending upgrade) consumes no marker and is not on
    // trial, so it is never auto-rolled-back. The initial launch can also be
    // a trial if a swap completed in a prior lifetime and the container was
    // restarted before the upgrade-restart fired.
    let mut on_trial = crate::update::install::take_upgrade_staged();
    let mut child = spawn_and_track();
    let mut spawned_at = std::time::Instant::now();
    eprintln!("[supervise] worker started (pid {})", child.id());

    loop {
        let worker_pid = WORKER_PID.load(Ordering::SeqCst);
        let mut status: libc::c_int = 0;
        // Block until *any* child changes state. As PID 1 we reap every
        // reparented orphan here; only the worker's exit drives decisions.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };

        if pid == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue; // interrupted by a forwarded signal; retry
            }
            if errno == libc::ECHILD {
                // No children left — nothing more to supervise.
                std::process::exit(if TERMINATING.load(Ordering::SeqCst) {
                    0
                } else {
                    1
                });
            }
            eprintln!("[supervise][FATAL] waitpid failed (errno {errno})");
            std::process::exit(1);
        }

        if pid != worker_pid {
            // Reaped an orphan zombie; ignore and keep going.
            continue;
        }

        // The worker exited. Decode how.
        let _ = child.try_wait(); // let std reap its own bookkeeping (no-op)
        // Clear the pid immediately: during the relaunch delay below a
        // forwarded signal must not `kill()` a PID the OS may have recycled.
        WORKER_PID.store(0, Ordering::SeqCst);
        if TERMINATING.load(Ordering::SeqCst) {
            eprintln!("[supervise] worker stopped during shutdown; exiting");
            std::process::exit(0);
        }

        let elapsed = spawned_at.elapsed();

        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            if code == runtime::EXIT_RESTART {
                eprintln!("[supervise] upgrade restart requested; relaunching in {delay}ms");
                std::thread::sleep(std::time::Duration::from_millis(delay));
                if TERMINATING.load(Ordering::SeqCst) {
                    std::process::exit(0);
                }
                // On trial only if an upgrade was actually staged (consumes
                // the marker); a plain restart leaves no marker and relaunches
                // without probation.
                on_trial = crate::update::install::take_upgrade_staged();
                child = spawn_and_track();
                spawned_at = std::time::Instant::now();
                eprintln!("[supervise] worker relaunched (pid {})", child.id());
                continue;
            }
            // A freshly-upgraded worker that dies fast can't boot — restore
            // the previous version and relaunch it (once; the restored worker
            // is no longer on_trial, so a second failure propagates).
            if on_trial
                && elapsed < trial
                && attempt_rollback(&format!(
                    "new version exited (code {code}) after {:.1}s",
                    elapsed.as_secs_f64()
                ))
            {
                on_trial = false;
                child = spawn_and_track();
                spawned_at = std::time::Instant::now();
                eprintln!("[supervise] previous version relaunched (pid {})", child.id());
                continue;
            }
            eprintln!("[supervise] worker exited with code {code}; propagating");
            std::process::exit(code);
        }

        if libc::WIFSIGNALED(status) {
            let sig = libc::WTERMSIG(status);
            if on_trial
                && elapsed < trial
                && attempt_rollback(&format!(
                    "new version was killed by signal {sig} after {:.1}s",
                    elapsed.as_secs_f64()
                ))
            {
                on_trial = false;
                child = spawn_and_track();
                spawned_at = std::time::Instant::now();
                eprintln!("[supervise] previous version relaunched (pid {})", child.id());
                continue;
            }
            eprintln!("[supervise] worker killed by signal {sig}; exiting");
            // Mirror the conventional 128+signal exit code.
            std::process::exit(128 + sig);
        }

        // Stopped/continued or otherwise non-terminal — keep waiting.
    }
}

#[cfg(windows)]
pub fn run() -> ! {
    // Windows has no PID 1 / zombie semantics. A straightforward
    // spawn → wait → relaunch loop is enough; standalone Windows servers
    // typically also sit under a service manager.
    let exe = runtime::self_exe();
    let args = worker_args();
    let delay = runtime::restart_delay_ms();
    let trial = std::time::Duration::from_secs(runtime::upgrade_trial_secs());
    // On trial only if a swap was actually staged at this launch (see the unix
    // path for the full rationale); a plain restart is never auto-rolled-back.
    let mut on_trial = crate::update::install::take_upgrade_staged();

    loop {
        let spawned_at = std::time::Instant::now();
        let mut child = std::process::Command::new(&exe)
            .args(&args)
            .env(runtime::ENV_SUPERVISED, "1")
            .env(runtime::ENV_RESTART_DELAY_MS, delay.to_string())
            .spawn()
            .unwrap_or_else(|e| {
                eprintln!("[supervise][FATAL] failed to spawn worker: {e}");
                std::process::exit(1);
            });

        let status = child.wait().unwrap_or_else(|e| {
            eprintln!("[supervise][FATAL] wait failed: {e}");
            std::process::exit(1);
        });
        let elapsed = spawned_at.elapsed();

        match status.code() {
            Some(code) if code == runtime::EXIT_RESTART => {
                eprintln!("[supervise] upgrade restart requested; relaunching in {delay}ms");
                std::thread::sleep(std::time::Duration::from_millis(delay));
                // Probation for the next launch only if an upgrade was staged.
                on_trial = crate::update::install::take_upgrade_staged();
            }
            Some(code) => {
                // A freshly-upgraded worker that dies fast can't boot —
                // restore the previous version and relaunch it once.
                if on_trial
                    && elapsed < trial
                    && attempt_rollback(&format!(
                        "new version exited (code {code}) after {:.1}s",
                        elapsed.as_secs_f64()
                    ))
                {
                    on_trial = false;
                    eprintln!("[supervise] relaunching previous version");
                    continue;
                }
                std::process::exit(code);
            }
            None => {
                if on_trial
                    && elapsed < trial
                    && attempt_rollback("new version terminated abnormally")
                {
                    on_trial = false;
                    eprintln!("[supervise] relaunching previous version");
                    continue;
                }
                std::process::exit(1);
            }
        }
    }
}
