//! The task system as Helio's task host (`helio_tasks::TaskHost`).
//!
//! Helio and the plugins' feature and world crates cannot depend on this
//! crate, so they start their threads and jobs through `helio_tasks`. The
//! engine installs [`install`] at startup: from then on each such thread is
//! a persistent task here (listed with its role and owner, a heartbeat, a
//! log) and each job a blocking task on the dedicated pool.

use crate::threads::{ThreadRole, ThreadSpec};
use crate::{
    global, ServiceDescription, ServiceHandle, TaskContext, TaskDescription, TaskDuration,
};
use std::sync::Arc;

/// Runs Helio-layer threads and jobs as tasks of the global queue.
pub struct EditorTaskHost;

/// Makes the global queue Helio's task host. Call once at startup, before
/// any renderer or voxel world starts; later calls do nothing.
pub fn install() {
    let _ = helio_tasks::install(Arc::new(EditorTaskHost));
}

struct Hooks(TaskContext);

impl helio_tasks::ContextHooks for Hooks {
    fn heartbeat(&self) {
        self.0.heartbeat();
    }

    fn set_idle(&self, idle: bool) {
        self.0.set_idle(idle);
    }

    fn set_phase(&self, phase: &str) {
        self.0.set_phase(phase);
    }

    fn stop_requested(&self) -> bool {
        self.0.is_cancelled()
    }
}

fn context(ctx: &TaskContext) -> helio_tasks::TaskContext {
    helio_tasks::TaskContext::new(Arc::new(Hooks(ctx.clone())))
}

struct Join(ServiceHandle<()>);

impl helio_tasks::ServiceJoin for Join {
    fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    fn request_stop(&self) {
        self.0.request_stop();
    }

    fn join(self: Box<Self>) -> std::thread::Result<()> {
        self.0.join()
    }
}

fn role(role: helio_tasks::Role) -> ThreadRole {
    match role {
        helio_tasks::Role::Service => ThreadRole::Service,
        helio_tasks::Role::Compute => ThreadRole::Compute,
        helio_tasks::Role::Io => ThreadRole::Io,
    }
}

impl helio_tasks::TaskHost for EditorTaskHost {
    fn spawn_service(
        &self,
        spec: helio_tasks::ServiceSpec,
        work: helio_tasks::ServiceWork,
    ) -> std::io::Result<helio_tasks::Service> {
        let mut thread = ThreadSpec::new(spec.thread, role(spec.role)).owner(spec.owner);
        if let Some(pool) = spec.pool {
            thread = thread.pool(pool);
        }
        if let Some(bytes) = spec.stack_size {
            thread = thread.stack_size(bytes);
        }
        let mut description = ServiceDescription::new(
            spec.title,
            spec.category,
            String::new(),
            ThreadRole::Service,
        );
        description.thread = thread;
        description.heartbeat = spec.heartbeat;
        let handle = global().spawn_service(description, move |ctx| work(&context(ctx)))?;
        Ok(helio_tasks::Service::new(Box::new(Join(handle))))
    }

    fn submit(&self, spec: helio_tasks::JobSpec, work: helio_tasks::JobWork) {
        global().submit(
            TaskDescription::new(spec.title, spec.category, TaskDuration::Long),
            move |ctx| {
                work(&context(&ctx));
                Ok(())
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn helio_services_and_jobs_are_tasks() {
        let host = EditorTaskHost;
        let (jobs, inbox) = mpsc::channel::<u32>();
        let (outbox, results) = mpsc::channel();
        let service = helio_tasks::TaskHost::spawn_service(
            &host,
            helio_tasks::ServiceSpec::new("Helio test service", "helio-test-service")
                .owner("tests"),
            Box::new(move |ctx: &helio_tasks::TaskContext| {
                while let Ok(n) = ctx.wait(|| inbox.recv()) {
                    outbox.send(n + 1).unwrap();
                }
            }),
        )
        .unwrap();
        jobs.send(1).unwrap();
        assert_eq!(results.recv().unwrap(), 2);
        assert!(global()
            .snapshots()
            .iter()
            .any(|task| task.title == "Helio test service"));
        let (done, finished) = mpsc::channel();
        helio_tasks::TaskHost::submit(
            &host,
            helio_tasks::JobSpec::new("Helio test job"),
            Box::new(move |_| done.send(()).unwrap()),
        );
        finished.recv().unwrap();
        assert!(global()
            .snapshots()
            .iter()
            .any(|task| task.title == "Helio test job"));
        drop(jobs);
        service.join().unwrap();
    }
}
