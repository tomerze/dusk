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

    #[cfg(test)]
    pub fn loads(&self) -> Vec<usize> {
        self.workers
            .iter()
            .map(|worker| worker.load.load(Ordering::SeqCst))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "the condition never held");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn test_clients_share_the_workers_instead_of_spawning_threads() {
        let pool = Pool::new(2).unwrap();
        let threads = Arc::new(Mutex::new(HashSet::new()));
        let started = Arc::new(AtomicUsize::new(0));
        let (release, _) = tokio::sync::broadcast::channel::<()>(1);
        for _ in 0..64 {
            let threads = threads.clone();
            let started = started.clone();
            let mut released = release.subscribe();
            pool.spawn(move || async move {
                let current = std::thread::current();
                threads
                    .lock()
                    .unwrap()
                    .insert((current.id(), current.name().map(String::from)));
                started.fetch_add(1, Ordering::SeqCst);
                if let Err(error) = released.recv().await {
                    panic!("the test never released the client: {error}");
                }
            })
            .unwrap();
        }
        wait_until(|| started.load(Ordering::SeqCst) == 64);
        let threads = threads.lock().unwrap().clone();
        assert_eq!(threads.len(), 2, "{threads:?}");
        assert!(
            threads.iter().all(|(_, name)| name
                .as_deref()
                .is_some_and(|name| name.starts_with("dusk-py-"))),
            "{threads:?}"
        );
        assert_eq!(pool.loads(), vec![32, 32]);
        release.send(()).unwrap();
        wait_until(|| pool.loads() == vec![0, 0]);
    }

    #[test]
    fn test_a_client_goes_to_the_least_loaded_worker() {
        let pool = Pool::new(3).unwrap();
        let (release, _) = tokio::sync::broadcast::channel::<()>(1);
        for _ in 0..2 {
            let mut released = release.subscribe();
            pool.spawn(move || async move {
                if let Err(error) = released.recv().await {
                    panic!("the test never released the client: {error}");
                }
            })
            .unwrap();
        }
        wait_until(|| pool.loads().iter().sum::<usize>() == 2);
        let mut loads = pool.loads();
        loads.sort();
        assert_eq!(loads, vec![0, 1, 1]);
        release.send(()).unwrap();
        wait_until(|| pool.loads() == vec![0, 0, 0]);
    }

    #[test]
    fn test_the_pool_size_comes_from_the_environment_value() {
        assert_eq!(size_from(Some(String::from("4"))), Ok(4));
        assert_eq!(size_from(Some(String::from(" 2 "))), Ok(2));
        assert!(size_from(None).unwrap() >= 1);
        for value in ["0", "-1", "many", ""] {
            assert!(
                size_from(Some(String::from(value))).is_err(),
                "{value:?} was accepted"
            );
        }
    }
}
