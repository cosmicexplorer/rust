use core::num::NonZero;

use crate::iter::adapters::zip::try_get_unchecked;
use crate::iter::adapters::{SourceIter, TrustedRandomAccess, TrustedRandomAccessNoCoerce};
use crate::iter::{FusedIterator, InPlaceIterable, TrustedLen, UncheckedIterator};
use crate::marker::Destruct;
use crate::ops::Try;

/// An iterator that clones the elements of an underlying iterator.
///
/// This `struct` is created by the [`cloned`] method on [`Iterator`]. See its
/// documentation for more.
///
/// [`cloned`]: Iterator::cloned
/// [`Iterator`]: trait.Iterator.html
#[stable(feature = "iter_cloned", since = "1.1.0")]
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Clone, Debug)]
pub struct Cloned<I> {
    it: I,
}

impl<I> Cloned<I> {
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(in crate::iter) const fn new(it: I) -> Cloned<I> {
        Cloned { it }
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
const fn clone_try_fold<T: [const] Clone, Acc, R>(
    mut f: impl [const] FnMut(Acc, T) -> R,
) -> impl [const] FnMut(Acc, &T) -> R {
    move |acc, elt| f(acc, elt.clone())
}

#[stable(feature = "iter_cloned", since = "1.1.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<'a, I, T: 'a> const Iterator for Cloned<I>
where
    I: [const] Iterator<Item = &'a T>,
    T: [const] Clone,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.it.next().cloned()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }

    fn try_fold<B, F, R>(&mut self, init: B, f: F) -> R
    where
        Self: Sized,
        F: [const] Destruct + [const] FnMut(B, Self::Item) -> R,
        R: [const] Try<Output = B>,
    {
        self.it.try_fold(init, clone_try_fold(f))
    }

    fn fold<Acc, F>(self, init: Acc, f: F) -> Acc
    where
        F: [const] Destruct + [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.it.map(T::clone).fold(init, f)
    }

    unsafe fn __iterator_get_unchecked(&mut self, idx: usize) -> T
    where
        Self: [const] TrustedRandomAccessNoCoerce,
    {
        // SAFETY: the caller must uphold the contract for
        // `Iterator::__iterator_get_unchecked`.
        unsafe { try_get_unchecked(&mut self.it, idx).clone() }
    }
}

#[stable(feature = "iter_cloned", since = "1.1.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<'a, I, T: 'a> const DoubleEndedIterator for Cloned<I>
where
    I: [const] DoubleEndedIterator<Item = &'a T>,
    T: [const] Clone,
{
    fn next_back(&mut self) -> Option<T> {
        self.it.next_back().cloned()
    }

    fn try_rfold<B, F, R>(&mut self, init: B, f: F) -> R
    where
        Self: Sized,
        F: [const] Destruct + [const] FnMut(B, Self::Item) -> R,
        R: [const] Try<Output = B>,
    {
        self.it.try_rfold(init, clone_try_fold(f))
    }

    fn rfold<Acc, F>(self, init: Acc, f: F) -> Acc
    where
        F: [const] Destruct + [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.it.map(T::clone).rfold(init, f)
    }
}

#[stable(feature = "iter_cloned", since = "1.1.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<'a, I, T: 'a> const ExactSizeIterator for Cloned<I>
where
    I: [const] ExactSizeIterator<Item = &'a T>,
    T: [const] Clone,
{
    fn len(&self) -> usize {
        self.it.len()
    }

    fn is_empty(&self) -> bool {
        self.it.is_empty()
    }
}

#[stable(feature = "fused", since = "1.26.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<'a, I, T: 'a> const FusedIterator for Cloned<I>
where
    I: [const] FusedIterator<Item = &'a T>,
    T: [const] Clone,
{
}

#[doc(hidden)]
#[unstable(feature = "trusted_random_access", issue = "none")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I> const TrustedRandomAccess for Cloned<I> where I: [const] TrustedRandomAccess {}

#[doc(hidden)]
#[unstable(feature = "trusted_random_access", issue = "none")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I> const TrustedRandomAccessNoCoerce for Cloned<I>
where
    I: [const] TrustedRandomAccessNoCoerce,
{
    const MAY_HAVE_SIDE_EFFECT: bool = true;
}

#[unstable(feature = "trusted_len", issue = "37572")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<'a, I, T: 'a> const TrustedLen for Cloned<I>
where
    I: [const] TrustedLen<Item = &'a T>,
    T: [const] Clone,
{
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<'a, I, T: 'a> const UncheckedIterator for Cloned<I>
where
    I: [const] UncheckedIterator<Item = &'a T>,
    T: [const] Clone,
{
    unsafe fn next_unchecked(&mut self) -> T {
        // SAFETY: `Cloned` is 1:1 with the inner iterator, so if the caller promised
        // that there's an element left, the inner iterator has one too.
        let item = unsafe { self.it.next_unchecked() };
        item.clone()
    }
}

#[stable(feature = "default_iters", since = "1.70.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] Default> const Default for Cloned<I> {
    /// Creates a `Cloned` iterator from the default value of `I`
    /// ```
    /// # use core::slice;
    /// # use core::iter::Cloned;
    /// let iter: Cloned<slice::Iter<'_, u8>> = Default::default();
    /// assert_eq!(iter.len(), 0);
    /// ```
    fn default() -> Self {
        Self::new(Default::default())
    }
}

#[unstable(issue = "none", feature = "inplace_iteration")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I> const SourceIter for Cloned<I>
where
    I: [const] SourceIter,
{
    type Source = I::Source;

    #[inline]
    unsafe fn as_inner(&mut self) -> &mut I::Source {
        // SAFETY: unsafe function forwarding to unsafe function with the same requirements
        unsafe { SourceIter::as_inner(&mut self.it) }
    }
}

#[unstable(issue = "none", feature = "inplace_iteration")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I: [const] InPlaceIterable> const InPlaceIterable for Cloned<I> {
    const EXPAND_BY: Option<NonZero<usize>> = I::EXPAND_BY;
    const MERGE_BY: Option<NonZero<usize>> = I::MERGE_BY;
}
