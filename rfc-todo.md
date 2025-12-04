# [`is_{read,write}_vectored()`](https://github.com/rust-lang/rust/issues/69941)

Is there any particular point of contact and/or communication channel that I can follow up with to move this PR forward? A year and a half later, not having this method stabilized means the zip crate is still unable to rely on atomicity guarantees for non-contiguous writes, which risks leaving important metadata in an inconsistent state.

*[several minutes pass]*

After some thought, I believe I can identify two subtleties that contribute to the difficulty of making progress on this stabilization PR:
1. The part I actually want from "stabilization" is not the precise *interface* of `is_read_vectored()` in itself, but the *implementation* work in the stdlib which makes e.g. `fs::File::is_read_vectored()` return `true`, so I can trust that if that method returns `true`, I can *definitely* rely on atomicity guarantees.
    - The above actually identifies *yet another* problem with the current trait methods: **this feels like a textbook setup for a TOCTOU error.**
2. I think @workingjubilee's initial proposal (associated `bool` const) seems like the right answer now. Problems with the current solution which their proposal addresses:
    - The `&self` trait method introduces a lifetime that has no bearing on the return value of the function. This
        - "no bearing"
    -
    - This could lead to confusing compile errors, and may lead users to employ `unsafe` to overcome the issue (since the lifetime was "incorrectly" imposed in the first place, using `unsafe` to remove that false claim may even be correct, despite the associated risks).
    - and completely precludes any type-level logic.
trait object with `where Self: Sized`
