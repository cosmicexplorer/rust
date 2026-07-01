use crate::fmt;
use crate::iter::{Fuse, FusedIterator};
use crate::marker::Destruct;

/// An iterator adapter that places a separator between all elements.
///
/// This `struct` is created by [`Iterator::intersperse`]. See its documentation
/// for more information.
#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[derive(Debug, Clone)]
pub struct Intersperse<I: Iterator>
where
    I::Item: Clone,
{
    started: bool,
    separator: I::Item,
    next_item: Option<I::Item>,
    iter: Fuse<I>,
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I> const FusedIterator for Intersperse<I>
where
    I: [const] FusedIterator,
    I::Item: [const] Clone,
{
}

impl<I: [const] Iterator> Intersperse<I>
where
    I::Item: [const] Clone,
{
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(in crate::iter) const fn new(iter: I, separator: I::Item) -> Self {
        Self { started: false, separator, next_item: None, iter: iter.fuse() }
    }
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I> const Iterator for Intersperse<I>
where
    I: [const] Iterator,
    I::Item: [const] Clone,
{
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.started {
            if let Some(v) = self.next_item.take() {
                Some(v)
            } else {
                let next_item = self.iter.next();
                if next_item.is_some() {
                    self.next_item = next_item;
                    Some(self.separator.clone())
                } else {
                    None
                }
            }
        } else {
            self.started = true;
            self.iter.next()
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        intersperse_size_hint(&self.iter, self.started, self.next_item.is_some())
    }

    fn fold<B, F>(self, init: B, f: F) -> B
    where
        Self: Sized,
        F: [const] Destruct + [const] FnMut(B, Self::Item) -> B,
    {
        let separator = self.separator;
        intersperse_fold(
            self.iter,
            init,
            f,
            move || separator.clone(),
            self.started,
            self.next_item,
        )
    }
}

/// An iterator adapter that places a separator between all elements.
///
/// This `struct` is created by [`Iterator::intersperse_with`]. See its
/// documentation for more information.
#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
pub struct IntersperseWith<I, G>
where
    I: Iterator,
{
    started: bool,
    separator: G,
    next_item: Option<I::Item>,
    iter: Fuse<I>,
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, G> const FusedIterator for IntersperseWith<I, G>
where
    I: [const] FusedIterator,
    G: [const] FnMut() -> I::Item,
{
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
impl<I, G> fmt::Debug for IntersperseWith<I, G>
where
    I: Iterator + fmt::Debug,
    I::Item: fmt::Debug,
    G: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntersperseWith")
            .field("started", &self.started)
            .field("separator", &self.separator)
            .field("iter", &self.iter)
            .field("next_item", &self.next_item)
            .finish()
    }
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, G> const Clone for IntersperseWith<I, G>
where
    I: [const] Iterator + [const] Clone,
    I::Item: [const] Clone,
    G: [const] Clone,
{
    fn clone(&self) -> Self {
        Self {
            started: self.started,
            separator: self.separator.clone(),
            iter: self.iter.clone(),
            next_item: self.next_item.clone(),
        }
    }
}

impl<I, G> IntersperseWith<I, G>
where
    I: [const] Iterator,
    G: [const] FnMut() -> I::Item,
{
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(in crate::iter) const fn new(iter: I, separator: G) -> Self {
        Self { started: false, separator, next_item: None, iter: iter.fuse() }
    }
}

#[unstable(feature = "iter_intersperse", reason = "recently added", issue = "79524")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, G> const Iterator for IntersperseWith<I, G>
where
    I: [const] Iterator,
    G: [const] FnMut() -> I::Item,
{
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.started {
            if let Some(v) = self.next_item.take() {
                Some(v)
            } else {
                let next_item = self.iter.next();
                if next_item.is_some() {
                    self.next_item = next_item;
                    Some((self.separator)())
                } else {
                    None
                }
            }
        } else {
            self.started = true;
            self.iter.next()
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        intersperse_size_hint(&self.iter, self.started, self.next_item.is_some())
    }

    fn fold<B, F>(self, init: B, f: F) -> B
    where
        Self: Sized,
        F: [const] Destruct + [const] FnMut(B, Self::Item) -> B,
    {
        intersperse_fold(self.iter, init, f, self.separator, self.started, self.next_item)
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
const fn intersperse_size_hint<I>(
    iter: &I,
    started: bool,
    next_is_some: bool,
) -> (usize, Option<usize>)
where
    I: [const] Iterator,
{
    let (lo, hi) = iter.size_hint();
    (
        lo.saturating_sub(!started as usize)
            .saturating_add(next_is_some as usize)
            .saturating_add(lo),
        hi.and_then(|hi| {
            hi.saturating_sub(!started as usize)
                .saturating_add(next_is_some as usize)
                .checked_add(hi)
        }),
    )
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
const fn intersperse_fold<I, B, F, G>(
    mut iter: I,
    init: B,
    mut f: F,
    mut separator: G,
    started: bool,
    mut next_item: Option<I::Item>,
) -> B
where
    I: [const] Iterator,
    F: [const] Destruct + [const] FnMut(B, I::Item) -> B,
    G: [const] FnMut() -> I::Item,
{
    let mut accum = init;

    let first = if started {
        next_item.take()
    } else {
        let n = iter.next();
        // skip invoking fold() for empty iterators
        if n.is_none() {
            return accum;
        }
        n
    };
    if let Some(x) = first {
        accum = f(accum, x);
    }

    iter.fold(accum, |mut accum, x| {
        accum = f(accum, separator());
        accum = f(accum, x);
        accum
    })
}
