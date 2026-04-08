<img src="https://github.com/dhall-lang/dhall-lang/blob/master/img/dhall-logo.svg" width="600" alt="Dhall Logo">

Dhall is a programmable configuration language optimized for
maintainability.

You can think of Dhall as: JSON + functions + types + imports

Note that while Dhall is programmable, Dhall is not Turing-complete.  Many
of Dhall's features take advantage of this restriction to provide stronger
safety guarantees and more powerful tooling.

You can find more details about the language by visiting the official website:

* [https://dhall-lang.org](http://dhall-lang.org/)

# STATUS

This is an experimental fork of [Nadrieril/dhall-rust](https://github.com/Nadrieril/dhall-rust),
not production-ready. The goal is to experiment with customizable Dhall
runtimes — pluggable import resolution, custom builtin functions, and a
`no_std`-compatible core.

# `dhall-rust`

This is the Rust implementation of the Dhall configuration language.
It is meant to be used to integrate Dhall in your application.

If you only want to convert Dhall to/from JSON or YAML, you should use the
official tooling instead; instructions can be found
[here](https://docs.dhall-lang.org/tutorials/Getting-started_Generate-JSON-or-YAML.html).

## Usage (at your own risk)

There are two ways to integrate Dhall in your application:

- **`serde_dhall`** — for straightforward deserialization of Dhall values into Rust types via serde.
- **`dhall_engine`** — for programmatic control: custom builtin functions, pluggable import resolution, and lazy evaluation.

### `serde_dhall`

Add this to your `Cargo.toml`:

```toml
[dependencies]
serde_dhall = "0.13.0"
```

Reading Dhall files is easy and leverages the wonderful [`serde`](https://crates.io/crates/serde) library.

```rust
use std::collections::BTreeMap;

// Some Dhall data
let data = "{ x = 1, y = 1 + 1 } : { x: Natural, y: Natural }";

// Deserialize it to a Rust type.
let deserialized_map: BTreeMap<String, u64> = serde_dhall::from_str(data).parse().unwrap();

let mut expected_map = BTreeMap::new();
expected_map.insert("x".to_string(), 1);
expected_map.insert("y".to_string(), 2);

assert_eq!(deserialized_map, expected_map);
```

### `dhall_engine`

Add this to your `Cargo.toml`:

```toml
[dependencies]
dhall_engine = { git = "https://github.com/Nachasic/dhall-rust" }
```

Register custom builtins that participate in typechecking and normalization:

```rust
use dhall_engine::{types::*, Engine};

struct DoubleNat;

impl<'cx> CustomBuiltinHandler<'cx> for DoubleNat {
    fn call(&self, args: &[Nir<'cx>], _cx: Ctxt<'cx>) -> Option<Nir<'cx>> {
        if args.len() != 1 { return None; }
        let n = u64::from_nir(&args[0])?;
        Some((n * 2).into_nir())
    }
}

let engine = Engine::new()
    .with_builtin("doubleNat", "Natural -> Natural", DoubleNat);

let result = engine.eval_str("doubleNat 21").unwrap();
assert_eq!(result.to_string(), "42");
```

`dhall` requires Rust >= 1.76.0

## Standard-compliance

This implementation currently supports most of the [Dhall
standard](https://github.com/dhall-lang/dhall-lang) version `20.0.0`.

The main missing feature is import headers. See
[here](https://github.com/Nadrieril/dhall-rust/issues?q=is%3Aopen+is%3Aissue+label%3Astandard-compliance)
for a list of the other missing features.

## Contributing

This section will cover how we can get started on contributing this project.

### Setting up the repository

To get a copy of this repository we can run:

```bash
$ git clone https://github.com/Nadrieril/dhall-rust.git
```

But we also might note that it's better practice to fork the repository to your own workspace.
There you can make changes and submit pull requests against this repository.

After the repositry has been cloned we need to update the [git submodule](https://git-scm.com/book/en/v2/Git-Tools-Submodules)
in the project, i.e. `dhall-lang`. We can do this by running:

```bash
$ git submodule update --init --recursive
```

### Building and Testing

A preferred method among the Rust community for developing is to use [`rustup`](https://rustup.rs/).

It can be installed by running:

```bash
$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

or if [nix](https://nixos.org/) is your tool of choice:

```bash
$ nix-shell -p rustup
```

Once `rustup` is installed we can get it to manage our toolchain by running:

```bash
$ rustup toolchain install stable
```

Then we can manage our building and testing with the [`cargo`](https://crates.io/) dependency manager:

```bash
$ cargo build
```

```bash
$ cargo test -- -q
```

You can also run tests individually by their name:

```bash
$ cargo test tests::spec::name_of_test
```

Now we can have fun and happy contributing!

### Test suite

The test suite uses tests from the dhall-lang submodule as well as from the
local `dhall/tests` directory.
The various tests are run according to the instructions present in
[`dhall-lang/tests/README.md`](https://github.com/dhall-lang/dhall-lang/blob/master/tests/README.md).

If an output test file (a `fooB.dhall` file) is missing, we will generate it automatically.
This is useful when writing new tests. Don't forget to commit it to git !

If one of the specification tests fails but you prefer the new output, you can
run the test(s) with `--bless` to overwrite the result file with the new
output. This happens often with ui tests (see below), since we may want to
change the phrasing of errors for example. Note that the `--bless` argument is
only accepted by the `spec` tests and will not be recognized if you also run
other test.

```bash
$ cargo test --test spec -- -q --bless
```

In addition to the usual dhall tests, we additionally run "ui tests", that
ensure that the output of the various errors stays good.
The output of the ui tests is stored in the local `dhall/tests` directory, even
for the tests coming from dhall-lang. They are stored in a `.txt` file with the
same name as the corresponding test.

### Commit messages

I try to keep commit messages somewhat in the style of [Conventional
Commits](https://www.conventionalcommits.org/en/v1.0.0). That means the commit
message should start with `feat:`, `test:`, `spec:`, `doc:`, `fix:`, `style:`,
`refactor:`, `chore:`, `perf:` or similar prefixes.

A breaking change should be indicated with `!` before the `:`.


## [Changelog](CHANGELOG.md)

## License

Licensed under the terms of the 2-Clause BSD License ([LICENSE](LICENSE) or
https://opensource.org/licenses/BSD-2-Clause)
