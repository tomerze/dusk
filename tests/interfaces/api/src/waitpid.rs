use dusk_program::embassy_executor::Executor;
use dusk_program::embassy_futures::block_on;
use dusk_program::handle;
use dusk_program::launcher_set::LauncherSet;
use dusk_program::namespace::Namespace;
use dusk_program_ps::{Args as PsArgs, Launcher as PsLauncher};
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

const FIXED_PID: u64 = 4242;

#[test]
fn test_waitpid_leaves_a_newer_process_at_the_same_pid() {
    Box::leak(Box::new(Executor::new())).run_until(
        |spawner| {
            let namespace = Rc::new(Namespace::new(handle::new_handle(), 0, spawner, None));
            let launcher_set = LauncherSet::from_launchers(vec![Box::new(PsLauncher::new())]);
            block_on(async {
                let program_args = PsArgs::new(None).as_program_args().unwrap();
                program_args.set_pid(Some(FIXED_PID)).unwrap();
                namespace
                    .clone()
                    .process(launcher_set.clone(), program_args)
                    .await
                    .unwrap();

                let dusk = dusk_core::local_client(namespace.clone()).await;
                let mut waitpid_request = dusk.waitpid_request();
                waitpid_request.get().set_pid(FIXED_PID);
                let mut waitpid = pin!(waitpid_request.send().promise);
                let waiting =
                    poll_fn(|context| Poll::Ready(waitpid.as_mut().poll(context).is_pending()))
                        .await;
                assert!(waiting, "waitpid waits while the process has not exited");

                namespace.exit(FIXED_PID, Ok(())).await;
                let program_args = PsArgs::new(None).as_program_args().unwrap();
                program_args.set_pid(Some(FIXED_PID)).unwrap();
                namespace
                    .clone()
                    .process(launcher_set, program_args)
                    .await
                    .unwrap();

                waitpid.await.unwrap();
                assert_eq!(
                    namespace.exited(FIXED_PID).await,
                    Some(false),
                    "the process created at pid {FIXED_PID} after the one waitpid waited on \
                     must still be in the table, not exited"
                );
            });
        },
        || true,
    );
}
