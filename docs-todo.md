- [ ] rename `INSTALL.md` => `BUILD.md`
  - https://github.com/rust-lang/rust/pull/119189
- [ ] remove the obnoxious language
> Empowering everyone to build reliable and efficient software.
yet
> Note: This document describes *building* Rust from *source*. This is *not recommended* if you don't know what you're doing. If you just want to install Rust, check out the README.md instead.

- [ ] the first sentence doesn't make sense

> The Rust build system uses a Python script called `x.py` to build the compiler, which manages the bootstrapping process.

think this should be:

`x.py`:
(a) downloads the prior nightly rustc/cargo from CI,
(b) modifies their RPATH in an extremely complex way (by hand, with very confusing dispatch based on platform),
(c) then uses those to compile the `bootstrap` tool, and
(d) `bootstrap` manages the bootstrapping process.

- [ ] "Dependencies" section should go before the `x.py` description!

- [ ] cargo has a flag for building with openssl

> To build Cargo, you'll also need OpenSSL (`libssl-dev` or `openssl-devel` on most Unix distros).

- [ ] cross-compiling "may need" = ?

> (when building for the host, `cc` is enough; cross-compiling may need additional compilers)

- [ ] ninja recommended why

> (Ninja is recommended, especially on Windows)

- [ ] `src/bootstrap/defaults/README.md` -- experimental?

> They are still experimental, and we'd appreciate your help improving them!

- [ ] `download-rustc` is broken

https://github.com/rust-lang/rust/issues/142505
