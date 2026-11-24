//! Managed install of a language server's executable.

use anyhow::Result;
use futures::{select_biased, FutureExt as _};
use gpui::Context;

use crate::session::Session;
use pulsar_language::spec::{InstallContext, LocateError, ServerInstaller};

use super::*;

impl LanguageServer {
    /// Whether this server's spec offers a managed install.
    pub fn installer(&self) -> Option<&'static ServerInstaller> {
        self.spec.installer
    }

    /// Whether an install is running now.
    pub fn is_installing(&self) -> bool {
        self.installing
    }

    /// Run the spec's managed install, then start the server. Only ever called
    /// for a person's explicit action (the footer's Install item): the host
    /// never installs on its own. The install is an editor task (Tasks window,
    /// its own log); the footer shows what it reports. A failure is the
    /// server's error status, with the install's message.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(installer) = self.spec.installer else {
            return;
        };
        if self.installing || self.is_running() {
            return;
        }
        self.installing = true;
        self.generation += 1;
        let events = self.session.starting();
        for event in events {
            cx.emit(event);
        }
        let name = self.spec.display_name;
        let (tx, rx) = flume::unbounded::<Result<(), LocateError>>();
        let (detail_tx, detail_rx) = flume::unbounded::<String>();
        let description = editor_task_queue::TaskDescription::new(
            format!("Install {name}"),
            "language-server",
            editor_task_queue::TaskDuration::Long,
        );
        editor_task_queue::global().submit(description, move |task| {
            task.set_phase("Installing");
            let progress = |text: &str| {
                tracing::info!("{text}");
                let _ = detail_tx.send(text.to_string());
            };
            let result = (installer.install)(&InstallContext::new(&progress));
            let outcome = match &result {
                Ok(()) => Ok(()),
                Err(error) => Err(error.message.clone()),
            };
            let _ = tx.send(result);
            outcome
        });
        self._install_pump = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = select_biased! {
                    text = detail_rx.recv_async().fuse() => Some(text),
                    result = rx.recv_async().fuse() => {
                        let result = result.unwrap_or_else(|_| {
                            Err(LocateError::new("the install ended without a result"))
                        });
                        let _ = this.update(cx, |this, cx| this.on_installed(result, cx));
                        return;
                    }
                };
                if let Some(Err(_)) = next {
                    // The install's progress channel closed: only its result is left.
                    let result = rx.recv_async().await.unwrap_or_else(|_| {
                        Err(LocateError::new("the install ended without a result"))
                    });
                    let _ = this.update(cx, |this, cx| this.on_installed(result, cx));
                    return;
                }
                if let Some(Ok(text)) = next {
                    let alive = this
                        .update(cx, |this, cx| {
                            this.detail = text.clone();
                            let events = this.session.starting_detail(&text);
                            this.emit_all(events, cx);
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
            }
        }));
        cx.notify();
    }

    fn on_installed(&mut self, result: Result<(), LocateError>, cx: &mut Context<Self>) {
        self.installing = false;
        match result {
            Ok(()) => {
                // Not running yet: the status is still the install's "Starting".
                self.session = Session::new(self.spec);
                self.start(cx);
            }
            Err(error) => {
                // Still not installed: the install is offered again.
                let events = self.session.missing(format!(
                    "The install of {} failed: {}",
                    self.spec.display_name, error.message
                ));
                self.emit_all(events, cx);
            }
        }
    }
}
