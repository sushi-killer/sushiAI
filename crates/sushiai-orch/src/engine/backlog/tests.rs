use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::queue::operations::Starter;
use crate::task::{Backlog, BacklogBucket, TaskStatus};
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct TaskListBarrier {
    path: PathBuf,
    payload: Vec<u8>,
    writer: Option<File>,
}

impl TaskListBarrier {
    fn block(path: PathBuf, payload: Vec<u8>) -> Self {
        std::fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        Self {
            path,
            payload,
            writer: None,
        }
    }

    fn wait_for_reader(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.path)
            {
                Ok(writer) => {
                    self.writer = Some(writer);
                    return;
                }
                Err(error)
                    if error.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline =>
                {
                    std::thread::yield_now();
                }
                Err(error) => panic!("start_task never reached its task-list read: {error}"),
            }
        }
    }
}

impl Drop for TaskListBarrier {
    fn drop(&mut self) {
        let writer = self.writer.take().or_else(|| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.path)
                .ok()
        });
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::write(&self.path, &self.payload);
        if let Some(mut writer) = writer {
            let _ = writer.write_all(&self.payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starting_a_task_keeps_an_owner_amendment_accepted_during_its_graph_read() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().criteria = vec!["old contract".into()];
    task.fixture_data().queue.backlog = Some(Backlog {
        bucket: BacklogBucket::Next,
        order: 0,
    });
    app.store.save_task(&task).unwrap();
    let other = task_with_status(TaskStatus::Done);
    app.store.save_task(&other).unwrap();
    let mut barrier = TaskListBarrier::block(
        app.store.task_path(&other.id),
        serde_json::to_vec(&other).unwrap(),
    );
    let signals = TaskSignals::new();
    signals.cancel.cancel();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(async {}),
        },
    );
    let runtime = tokio::runtime::Handle::current();
    let starter = {
        let (app, id) = (app.clone(), task.id.clone());
        std::thread::spawn(move || {
            let _runtime = runtime.enter();
            app.start_task(&id, Starter::Owner);
        })
    };
    barrier.wait_for_reader();
    let lock = app.task_lock(&task.id);
    let start_owns_task = lock.try_lock().is_err();
    let mut amendment = {
        let (app, id) = (app.clone(), task.id.clone());
        tokio::spawn(async move {
            app.dispatch(
                "task.amend",
                json!({"id": id, "criteria": ["accepted current owner contract"]}),
            )
            .await
        })
    };
    let accepted_before_release = if start_owns_task {
        None
    } else {
        Some(
            tokio::time::timeout(Duration::from_secs(10), &mut amendment)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
    };
    drop(barrier);
    starter.join().unwrap();
    let response = match accepted_before_release {
        Some(response) => response,
        None => tokio::time::timeout(Duration::from_secs(10), amendment)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    };
    assert_eq!(response["pending"], false);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["accepted current owner contract"]);
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(saved.queue.backlog.is_none());
    assert!(saved
        .decisions
        .iter()
        .any(|line| line == "Owner: started from the backlog"));
    assert!(saved.attempts.is_empty());
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}
