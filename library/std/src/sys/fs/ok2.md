I asked a friend how to enable `mold` and they told me `-fuse-ld=mold` and it didn't work! I wasn't sure why, and found this PR!

1. This PR repeatedly and unmistakably describes a single very specific incredibly narrow subset of the performance data as justification for the changes. This is a member of the rustc compile performance working area, their work has been sponsored by google and their various subsidiaries, and they make misleading and false statements in blog posts using the Rust letterhead.
2. Furthermore, the `rustc-perf` benchmark apparatus itself is designed to obscure and even encourage the usage of manipulated data.
3. The PR makes a much, much broader-reaching set of changes than identified in the title.
4. Multiple replies that would have immediately challenged the authority of the rust compile performance team were quickly responded to with word salad in the shape of invective, which received the appearance of broader support potential sock-puppet behavior through the github comment reaction interface.

# The `rust-lld` default has been entirely motivated by intentionally selecting laughably unrepresentative examples of the rust compile performance data

> The main motivation for using LLD instead of the default BFD linker is improving [compilation times](https://perf.rust-lang.org/compare.html?start=b3e117044c7f707293edc040edb93e7ec5f7040a&end=baed03c51a68376c1789cc373581eea0daf89967&stat=instructions%3Au&tab=compile). For example, in the linked benchmark, it makes incremental recompilation of `ripgrep` in `debug` more than twice faster.

So presumably you mean the `incr-patched: println` or `incr-unchanged` runs? Because `incr-full`  is less than twice faster. Let's look at the profile data for `incr-patched: println`: https://perf.rust-lang.org/detailed-query.html?commit=baed03c51a68376c1789cc373581eea0daf89967&benchmark=ripgrep-13.0.0-debug&scenario=incr-patched:%20println&base_commit=b3e117044c7f707293edc040edb93e7ec5f7040a

- The actual time differential was 0.62 seconds vs 1.4 seconds? And that time includes invoking a linker subprocess?
    - I don't think 0.62 seconds vs 1.4 seconds is a significant amount of compilation time saved?
- This blog post also never mentions wall-clock times once: https://blog.rust-lang.org/2024/05/17/enabling-rust-lld-on-linux/.
    - But it *does* mention the same ripgrep debug example. I don't understand how this is real????
- This blog post just says "noticeable": https://www.memorysafety.org/blog/remy-rakic-compile-times/
    - *Still* the same ripgrep debug example??????
    - This time it's 0.91 seconds to 2 seconds, because it's an incremental build without any compile work to do.

There is one further point to note here on the manipulation of the timing. As per OP:
**Timing comparisons limit the bfd linker's maximum threading, as well as induce a brief initial delay, because the gnu linker supports the jobserver protocol and lld does not.** As far as I am aware, this has always been the case.

# Discussion: why `rustc-perf` faills to capture meaningful trends, and is easy to slant

@Mark-Simulacrum: It would be a good idea to at least include the wall-clock time on the `compare.html` page for the perf suite, so that the URLs Rémy provided as "proof" would have had a "ground-truth" basis upon every comparison? This would also seem to imply that there's a lot of poor-quality and misleading data points in the perf suite more generally, if you're not attempting to clean the data at all. Jamie Jennings at NCSU makes this incredibly thoughtful tool for benchmarking process executions called bestguess: https://gitlab.com/JamieTheRiveter/bestguess#about-measurement-quality
> BestGuess uses times obtained via `wait4()`.  In other words, it has direct
> access to the process accounting done by the OS.  And BestGuess does no extra
> work after forking a new process but before executing the command.  Hyperfine,
> which uses a Rust process management crate, may do significant work in that gap
> between fork and exec.  That work that will accrue time to the measured
> command.

But that's just for removing some of the OS noise alone--it won't define noise.

For pants at Twitter we found percentiles and bucketing to be very very helpful visualizations. As a build performance engineer, I actually found p90 and p95 build times to be relatively robust and achievable OKRs for the 3 years I worked at Twitter Inc. It's definitely possible to actively fabricate data, but selecting a misleading view of the data is far more common.

@Kobzol @nnethercote @Jamesbarford @panstromek @Zoxc Could anyone on the compiler performance team help me to understand whether this blog post (by a compiler performance working area member, on the official Rust domain, with the official "Rust blog" letterhead) represents the consensus of the compiler performance team?
https://blog.rust-lang.org/2024/05/17/enabling-rust-lld-on-linux/

I personally find it fundamentally deceitful to begin a document with the phrasing:
> Linking time is often a big part of compilation time.

Then have an entire paragraph making vague claims without any citations implying the GNU ld on my machine is a relic from the single-core days. Then saying "for example":
> For example, when building ripgrep 13 in debug mode on Linux, roughly half of the time is actually spent in the linker.

This is just false unless referring to one of the incremental builds, in which there is almost no compile work to do. When the compilation isn't completely cached (like in `debug full`), linking takes **3.53%** of the runtime: https://perf.rust-lang.org/detailed-query.html?commit=baed03c51a68376c1789cc373581eea0daf89967&benchmark=ripgrep-13.0.0-debug&scenario=full&base_commit=b3e117044c7f707293edc040edb93e7ec5f7040a

## Linker time is artificially flattened

The `rustc-perf` effort presents an interface that on its face looks much like the output of a tracing profiler, which recurses into individual method calls. During the precise incremental runs used as the whole of the rationale for the changes in this PR, it can be seen that whhat *looks* like a profile appears to be in fact self-reported in some way.

Usually when I see `run_linker` at the top of my profile list with 20% of my runtime, my next step is to figure out why my profiler can't see into that. That is not a proof  that the linker was slower until you see what the linker was spending time on!

Again: I don't understand how this can come from someone claiming to be a performance engineer?

## There were concerns raised immediately.

> > One thing that may be worth noting is that the current design makes it harder to switch the default linker again in the future
>
> I think we all agree that the opt outs are expected to be used extremely rarely.

So

> If for some reason, the linker the rust developers have decided a target uses is not appropriate for a given project, regardless of which linker that is, then they can opt into the linker they know works for them instead. It's like opting out of lld or rust-lld via `-Clink-arg=-fuse-ld=bfd` today.


@lqd: you failed to include the conclusion of the post you replied to above. That part was:
> > This makes me wonder whether the design for this option shouldn't be more along the lines of "I want to use the system linker" rather than "I don't want to use lld".

Your response described an entirely separate proposal you appear to have made up on the spot. "Rather than" and other phrases indicated that the proposed change (which received 10 "thumbs up") would be *instead* of the lld approach. You also invoked the term "bikeshed", implying that this was an unimportant detail slowing down your ability to merge the change.

> But we can still add an additional universal opt out if you want, a `-Clinker-features=-bikeshed` that would be the future proof way to return to the system linker. That doesn't have to affect the existence of going from rust-lld to system lld (`-Clink-self-contained=-linker`), or to not use rust-lld or lld (`-Clinker-features=-lld`) -- these 2 features were explicitly asked for in the discussion for MCP510 to be accepted.

https://github.com/rust-lang/rust/pull/140525#issuecomment-2846721224


From OP:
> The implementation of linker flavors with LLD was causing a sort of a combinatorial explosion of various options.

Where was this observed? What effect did it have? There is no citation for this issue which motivated a radically new approach to linker selection. A "combinatorial explosion" typically refers to computational complexity. Here it appears to be referring to CLI option aesthetic value?

From https://doc.rust-lang.org/rustc/codegen-options/index.html#linker-features:
> These feature flags are a flexible extension mechanism that is complementary to linker flavors, designed to avoid the combinatorial explosion of having to create a new set of flavors for each linker feature we'd want to use.

In OP, you say that linker flavors are creating an explosion of options. In the docs, you say instead that rustc would otherwise need to "create a new set of flavors for each linker feature"?


https://github.com/rust-lang/rust/pull/119906 suggested a different approach for linker flavors (described https://github.com/rust-lang/rust/pull/119906#issuecomment-1894088306), where the individual flavors could be enabled separately using +/- (e.g. +lld).

https://github.com/rust-lang/rust/pull/119906#issuecomment-1894088306

> Linker flavor naming
> The base component is unfortunately needed

> It would be nice to just drop the base flavor components (like gnu/darwin/wasm/etc) from user-facing flavors and leave only generic ld, lld, (ld,lld)-(cc,clang), because the base flavor it tied to the target.
> This is not compatible with items 3 and 4 though - some targets like UEFI need linkers outside of their base flavor, and the hypothetical llvm-bc flavor should also be compatible with a wide range of targets.
> So we should have the full flavors in some form, but typically using the generic flavors seem preferable.


> LLD doesn't support the jobserver protocol for limiting the number of threads used, it simply defaults to using all available cores, and is one of the reasons why it's faster than BFD.

> When enabling rust-lld on nightly, we also switched x64 linux to use it at stage >= 1, meaning that all tests have been running with lld since May 2024, on CI as well as contributors' machines. (Post opt-dist tests also had been using it when running their test subset earlier than that).
