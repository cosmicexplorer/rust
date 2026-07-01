From reading the thread at https://internals.rust-lang.org/t/alternative-approach-to-zno-link-zlink-only-split-linking/14842, I have one brief comment and several less brief oness:

@bjorn3:
> Also various of the inputs of the link process don't exist in the crate metadata at all. Only the rlink files contain them.
> ...
> What I am trying to say is that using rlib is at least as complex as rlink and may even be more complex due to no way to communicate certain things.
> ...
> https://github.com/rust-lang/rust/pull/93681#issuecomment-1030845942
> [rlink] isn't meant to be read outside of rustc.

Synthesizing these comments of yours from 2021 and 2022 together, I now feel[^eugene] even more like "an explicitly public format, constructed from compiler-internal data, yet designed to be abstracted from compiler internals" in the manner of "SemanticDB[^sem-db] for rust" is such a tantalizing idea!

[^eugene]: https://github.com/rust-lang/rust/issues/67293#issuecomment-3696345567
[^sem-db]: https://scalameta.org/docs/semanticdb/guide.html#semanticdb-based-tools

@jsgf:
> Personally, I'm particularly interested in the possibilities of distributed builds, and aggressively caching and reusing build outputs.

Having actually shipped a highly parallel distributed build process to production for Scala builds at Twitter[^eugene] via working very closely with Eugene Burmako on co-engineering rsc[^rsc] and pants together for this purpose, caching and parallelization tend to reach their global maximum when you pay world-class researchers to build powerful systems in their field of expertise. Many people get this wrong and will only compensate engineers for constructing systems of enclosure and deskilling and then call the promotion ladder that valorizes stealing credit from people who devote their lives to constantly reinventing the state of the art a "free market".

[^rsc]: https://github.com/twitter/rsc

In particular, the epiphany that we shipped to Twitter engineers occurred not just from Eugene Burmako and Stu Hood's initial epiphany to enable pipelining with parallel scalac invocations as per Eugene's Scale by the Bay 2018 talk[^sbtb], but Win Wang and my extended codevelopment session which identified the relationship between the process lifetime distinction of the multithreaded local HotSpot JIT rsc invocation vs highly parallel ephemeral remote execution of graal `native-image` AOT-compiled scalac jobs, and how attempting to do pipelining with 100% remote execution was completely losing the benefit of pipelining in the first place by taking the network i/o hit both ways across the topologically sorted partial ordering of the outline compilation phase.

[^sbtb]: https://www.youtube.com/watch?app=desktop&v=8SnIBkJXD8I
[^bzrust]: https://github.com/bazelbuild/rules_rust/issues/428#issuecomment-3695407768

Eugene Burmako was not an expert on I/O latencies and resource saturation. I was not an expert on parallel typechecking. Win and I did not finally crack the case until we understood how computer performance is a result of bidirectional communication protocols suited to the task at hand.

> In the long term I'd like to find some way where rustc can provide enough information to completely construct an external link line, so that we don't have to rely on it to invoke the linker - ie so that a top-level C++ target can link in Rust code in a native way.

Constructing a link line and providing it to the build system to invoke reminds me of how the user provides their own `&mut [u8]` buffer to `std::io::Read` and submits it to the OS. Constructing a `.rlink` and postprocessing it into a standardized format (the aforementioned SemanticDB for rust) reminds me of adapters like `io::Chain`.

> Secondly since the `.rlink` file is a dump of internal compiler state, it must be treated as opaque. But as mentioned above, it contains a full set of paths to `.rlib` files that the executable depends on (directly and indirectly). These paths are absolute paths, which means that they're likely only meaningful on a single machine, implying that the `-Zno-link` and `-Zlink-only` phases can't be distributed.

The `.rlink` file being opaque already means it can't be distributed.

> This makes split linking look a lot more normal to the surrounding build system

It is the build system's job to look normal to the compiler.

> `-Zlink-only` should treat all its inputs as inputs and preserve them. Failing to do either of these could corrupt a cache

`chmod -w`

@adetaylor https://github.com/rust-lang/rust/issues/64191#issuecomment-622607220
> either stabilizing `.rlink` (which sounds unlikely) or having some official way to build a linker command line which is not under the control of rustc

Going from `.rlink` (a rustc-specific interface) directly to a linker command line (which is tied to at least your choice of linker) seems to forget the law of excluded middle! Building a linker command line sounds like it should be definitionally the job of the build system, should it not?

"under the control" lmao. The build system is executing the compiler as a subprocess. The compiler is not responsible for the build system's architectural constraints or internal politics. Build systems are not a tool to neg the compiler after a contributor does the one thing the build system can't do without forking it. This is a document I created in 2018: https://docs.google.com/document/d/1iWINqg30nXaCYmBxma5h5RgWzx7sgFqWGJcQJ-RYpRE/edit?usp=drive_link

> Compilation tends to be CPU-bound in most programming languages which have a nontrivial compilation step. Many other tools used during development, such as formatters and linters, perform much less computation per input file. Additionally, Scala and Java source files require their dependencies to be compiled beforehand, imposing a partial order, which formatters and linters typically don't require. This leads to a characterization of these operations as an example of an embarrassingly parallel IO-bound problem. While multithreading can be used to achieve parallelism and maintain JIT state, the implementation of threading in an individual command-line tool is unlikely to be optimal in all scenarios, and may conflict with attempts to further parallelize the tool invocation by a parent process.
>
> On the other hand, process-level parallelism can be finely controlled by a parent process without requiring any optimization on the part of the command-line tool. Furthermore, the parallelism can be effectively tuned when the amount of work is known in advance, and more IO-bound workloads can often be more accurately predicted by the size of the input files. A build tool such as Pants which invokes these subprocesses has all of this information before the run begins, and can tune the number of files each subprocess operates on, and even attempt to partition the input files into buckets of similar file size in an attempt to maximize the number of processes running in parallel. Finally, using multiple subprocesses allows the build tool to scale the parallelism arbitrarily high, which saturates the operating system with IO requests and allows the operating system to perform one of the jobs it excels at, which is orchestrating IO requests from many different processes to most efficiently make use of the underlying filesystem and hardware.
