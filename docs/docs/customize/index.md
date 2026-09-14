# Customize Dusk

Embedding gives you fleet management out of the box. When you need more, Dusk is
**modular all the way down** - and this is where you reach in.

This is the **third** way people use Dusk, after
[analytics](../features/analytics.md) and [diagnosis](../features/diagnosis.md),
and most never need it. Like nobody writes their own SSH server - though they
absolutely could - most fleets run the built-ins. But nothing here is locked
down:

- **Add your own programs** - new programs a node can run, built from the same
  five-part contract as `ps` and the shell. See
  [Write a program](../getting-started/guides/first-program.md).
- **Swap the platform backend** - a Dusk *impl* is what makes a node run on a
  given platform. Target new hardware without touching a single program. See
  [Build a custom impl](../getting-started/guides/custom-impl.md).
- **Choose what ships** - a node links exactly the programs you give it. Drop the
  Base set, extend it, or replace it entirely.

Start with the [Concepts](../getting-started/concepts/programs.md) - the model
behind a program and a node - then the guides above. This is the layer that makes
Dusk a framework, not just a drop-in.
