use dusk_program::anyhow::Result;

use crate::sh_capnp::script;
use dusk_capnp::dusk_capnp::dusk;
use dusk_capnp::dusk_capnp::stream;

mod execution;

#[derive(Clone)]
pub struct Interpreter {
    client: dusk::Client,
}

impl Interpreter {
    pub fn new(client: dusk::Client) -> Self {
        Interpreter { client }
    }

    pub async fn exec(&self, script: script::Reader<'_>, output: stream::Client) -> Result<()> {
        let statements = script.reborrow().get_statements()?;

        for statement in statements.iter() {
            let expr = statement.get_expr()?;
            let background = statement.get_background();
            let mode = match background {
                true => execution::Mode::Background(),
                false => execution::Mode::Output(output.clone()),
            };
            let execution = execution::Execution::new(self.client.clone(), mode);
            execution.exec_expr(expr).await?;
        }
        output.done_request().send().promise.await?;

        Ok(())
    }
}
