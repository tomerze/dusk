use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;

pub const WORKERS_VARIABLE: &str = "DUSK_PY_WORKERS";

const PENDING_STARTS: usize = 1024;

const BLOCKING_THREADS: usize = 64;

type Start = Box<dyn FnOnce() + Send>;

struct Worker {
    starts: mpsc::Sender<Start>,
    load: Arc<AtomicUsize>,
}

pub struct Pool {
    workers: Vec<Worker>,
}

struct Load(Arc<AtomicUsize>);

impl Drop for Load {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Pool {
    pub fn new(size: usize) -> std::io::Result<Self> {
        let mut workers = Vec::with_capacity(size);
        for index in 0..size {
            let (starts, mut receiver) = mpsc::channel::<Start>(PENDING_STARTS);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(BLOCKING_THREADS)
                .build()?;
            std::thread::Builder::new()
                .name(format!("dusk-py-{index}"))
                .spawn(move || {
                    let local = tokio::task::LocalSet::new();
                    local.block_on(&runtime, async move {
                        while let Some(start) = receiver.recv().await {
                            start();
                        }
                    });
                })?;
            workers.push(Worker {
                starts,
                load: Arc::new(AtomicUsize::new(0)),
            });
        }
        tracing::info!(workers = size, "started the client worker pool");
        Ok(Pool { workers })
    }

    pub fn spawn<Make, Task>(&self, make: Make) -> Result<(), String>
    where
        Make: FnOnce() -> Task + Send + 'static,
        Task: Future<Output = ()> + 'static,
    {
        let worker = self
            .workers
            .iter()
            .min_by_key(|worker| worker.load.load(Ordering::SeqCst))
            .ok_or_else(|| String::from("the client worker pool has no workers"))?;
        worker.load.fetch_add(1, Ordering::SeqCst);
        let load = Load(worker.load.clone());
        let start: Start = Box::new(move || {
            let task = make();
            let client = tokio::task::spawn_local(async move {
                let _load = load;
                task.await;
            });
            tokio::task::spawn_local(async move {
                if let Err(error) = client.await {
                    tracing::error!(error = %error, "a client's connection task panicked");
                }
            });
        });
        worker.starts.try_send(start).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                String::from("the client worker pool is overloaded, try again")
            }
            mpsc::error::TrySendError::Closed(_) => {
                String::from("a client worker has stopped, restart the process")
            }
        })
    }
}

fn size_from(value: Option<String>) -> Result<usize, String> {
    match value {
        None => Ok(std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)),
        Some(value) => match value.trim().parse::<usize>() {
            Ok(size) if size > 0 => Ok(size),
            _ => Err(format!(
                "{WORKERS_VARIABLE} is `{value}`, it has to be a whole number above 0"
            )),
        },
    }
}

static POOL: Mutex<Option<(u32, Arc<Pool>)>> = Mutex::new(None);

pub fn pool() -> Result<Arc<Pool>, String> {
    let mut current = POOL.lock().unwrap_or_else(PoisonError::into_inner);
    let process = std::process::id();
    if let Some((owner, pool)) = current.as_ref()
        && *owner == process
    {
        return Ok(pool.clone());
    }
    let size = size_from(std::env::var(WORKERS_VARIABLE).ok())?;
    let pool = Arc::new(
        Pool::new(size)
            .map_err(|error| format!("couldn't start the client worker pool: {error}"))?,
    );
    *current = Some((process, pool.clone()));
    Ok(pool)
}
