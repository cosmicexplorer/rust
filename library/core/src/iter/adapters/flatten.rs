use crate::iter::adapters::SourceIter;
use crate::iter::{
    Cloned, Copied, Empty, Filter, FilterMap, Fuse, FusedIterator, Map, Once, OnceWith,
    TrustedFused, TrustedLen,
};
use crate::marker::Destruct;
use crate::num::NonZero;
use crate::ops::{ControlFlow, Try};
use crate::{array, fmt, option, result};

/// An iterator that maps each element to an iterator, and yields the elements
/// of the produced iterators.
///
/// This `struct` is created by [`Iterator::flat_map`]. See its documentation
/// for more.
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[stable(feature = "rust1", since = "1.0.0")]
pub struct FlatMap<I, U: IntoIterator, F> {
    inner: FlattenCompat<Map<I, F>, <U as IntoIterator>::IntoIter>,
}

impl<I: Iterator, U: IntoIterator, F: FnMut(I::Item) -> U> FlatMap<I, U, F> {
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(in crate::iter) const fn new(iter: I, f: F) -> FlatMap<I, U, F> {
        FlatMap { inner: FlattenCompat::new(iter.map(f)) }
    }

    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(crate) const fn into_parts(self) -> (Option<U::IntoIter>, Option<I>, Option<U::IntoIter>)
    where
        U: [const] IntoIterator,
    {
        (
            self.inner.frontiter,
            self.inner.iter.into_inner().map(Map::into_inner),
            self.inner.backiter,
        )
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] Clone, U, F: [const] Destruct + [const] Clone> const Clone for FlatMap<I, U, F>
where
    U: [const] Clone + [const] IntoIterator<IntoIter: [const] Clone>,
{
    fn clone(&self) -> Self {
        FlatMap { inner: self.inner.clone() }
    }
}

#[stable(feature = "core_impl_debug", since = "1.9.0")]
impl<I: fmt::Debug, U, F> fmt::Debug for FlatMap<I, U, F>
where
    U: IntoIterator<IntoIter: fmt::Debug>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlatMap").field("inner", &self.inner).finish()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] Iterator, U: [const] IntoIterator, F> const Iterator for FlatMap<I, U, F>
where
    F: [const] Destruct + [const] FnMut(I::Item) -> U,
{
    type Item = U::Item;

    #[inline]
    fn next(&mut self) -> Option<U::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    #[inline]
    fn try_fold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.inner.try_fold(init, fold)
    }

    #[inline]
    fn fold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.inner.fold(init, fold)
    }

    #[inline]
    fn advance_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        self.inner.advance_by(n)
    }

    #[inline]
    fn count(self) -> usize {
        self.inner.count()
    }

    #[inline]
    fn last(self) -> Option<Self::Item> {
        self.inner.last()
    }
}

#[stable(feature = "rust1", since = "1.0.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] DoubleEndedIterator, U, F> const DoubleEndedIterator for FlatMap<I, U, F>
where
    F: [const] Destruct + [const] FnMut(I::Item) -> U,
    U: [const] IntoIterator<IntoIter: [const] DoubleEndedIterator>,
{
    #[inline]
    fn next_back(&mut self) -> Option<U::Item> {
        self.inner.next_back()
    }

    #[inline]
    fn try_rfold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.inner.try_rfold(init, fold)
    }

    #[inline]
    fn rfold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.inner.rfold(init, fold)
    }

    #[inline]
    fn advance_back_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        self.inner.advance_back_by(n)
    }
}

#[stable(feature = "fused", since = "1.26.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U, F> const FusedIterator for FlatMap<I, U, F>
where
    I: [const] FusedIterator,
    U: [const] IntoIterator,
    F: [const] Destruct + [const] FnMut(I::Item) -> U,
{
}

#[unstable(feature = "trusted_len", issue = "37572")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I, U, F> const TrustedLen for FlatMap<I, U, F>
where
    I: [const] Iterator,
    U: [const] IntoIterator,
    F: [const] Destruct + [const] FnMut(I::Item) -> U,
    FlattenCompat<Map<I, F>, <U as IntoIterator>::IntoIter>: [const] TrustedLen,
{
}

#[unstable(issue = "none", feature = "inplace_iteration")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I, U, F> const SourceIter for FlatMap<I, U, F>
where
    I: [const] SourceIter + [const] TrustedFused,
    U: [const] IntoIterator,
{
    type Source = I::Source;

    #[inline]
    unsafe fn as_inner(&mut self) -> &mut I::Source {
        // SAFETY: unsafe function forwarding to unsafe function with the same requirements
        unsafe { SourceIter::as_inner(&mut self.inner.iter) }
    }
}

/// An iterator that flattens one level of nesting in an iterator of things
/// that can be turned into iterators.
///
/// This `struct` is created by the [`flatten`] method on [`Iterator`]. See its
/// documentation for more.
///
/// [`flatten`]: Iterator::flatten()
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[stable(feature = "iterator_flatten", since = "1.29.0")]
pub struct Flatten<I: Iterator<Item: IntoIterator>> {
    inner: FlattenCompat<I, <I::Item as IntoIterator>::IntoIter>,
}

impl<I: Iterator<Item: IntoIterator>> Flatten<I> {
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    pub(in super::super) const fn new(iter: I) -> Flatten<I> {
        Flatten { inner: FlattenCompat::new(iter) }
    }
}

#[stable(feature = "iterator_flatten", since = "1.29.0")]
impl<I, U> fmt::Debug for Flatten<I>
where
    I: fmt::Debug + Iterator<Item: IntoIterator<IntoIter = U, Item = U::Item>>,
    U: fmt::Debug + Iterator,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flatten").field("inner", &self.inner).finish()
    }
}

#[stable(feature = "iterator_flatten", since = "1.29.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const Clone for Flatten<I>
where
    I: [const] Clone + [const] Iterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] Clone + [const] Iterator,
{
    fn clone(&self) -> Self {
        Flatten { inner: self.inner.clone() }
    }
}

#[stable(feature = "iterator_flatten", since = "1.29.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const Iterator for Flatten<I>
where
    I: [const] Iterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] Iterator,
{
    type Item = U::Item;

    #[inline]
    fn next(&mut self) -> Option<U::Item> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    #[inline]
    fn try_fold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.inner.try_fold(init, fold)
    }

    #[inline]
    fn fold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.inner.fold(init, fold)
    }

    #[inline]
    fn advance_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        self.inner.advance_by(n)
    }

    #[inline]
    fn count(self) -> usize {
        self.inner.count()
    }

    #[inline]
    fn last(self) -> Option<Self::Item> {
        self.inner.last()
    }
}

#[stable(feature = "iterator_flatten", since = "1.29.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const DoubleEndedIterator for Flatten<I>
where
    I: [const] DoubleEndedIterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] DoubleEndedIterator,
{
    #[inline]
    fn next_back(&mut self) -> Option<U::Item> {
        self.inner.next_back()
    }

    #[inline]
    fn try_rfold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.inner.try_rfold(init, fold)
    }

    #[inline]
    fn rfold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.inner.rfold(init, fold)
    }

    #[inline]
    fn advance_back_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        self.inner.advance_back_by(n)
    }
}

#[stable(feature = "iterator_flatten", since = "1.29.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const FusedIterator for Flatten<I>
where
    I: [const] FusedIterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] Iterator,
{
}

#[unstable(feature = "trusted_len", issue = "37572")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I> const TrustedLen for Flatten<I>
where
    I: [const] Iterator<Item: [const] IntoIterator>,
    FlattenCompat<I, <I::Item as IntoIterator>::IntoIter>: [const] TrustedLen,
{
}

#[unstable(issue = "none", feature = "inplace_iteration")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<I> const SourceIter for Flatten<I>
where
    I: [const] SourceIter + [const] TrustedFused + [const] Iterator,
    <I as Iterator>::Item: [const] IntoIterator,
{
    type Source = I::Source;

    #[inline]
    unsafe fn as_inner(&mut self) -> &mut I::Source {
        // SAFETY: unsafe function forwarding to unsafe function with the same requirements
        unsafe { SourceIter::as_inner(&mut self.inner.iter) }
    }
}

#[stable(feature = "default_iters", since = "1.70.0")]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I> const Default for Flatten<I>
where
    I: [const] Default + [const] Iterator<Item: [const] IntoIterator>,
{
    /// Creates a `Flatten` iterator from the default value of `I`.
    ///
    /// ```
    /// # use core::slice;
    /// # use std::iter::Flatten;
    /// let iter: Flatten<slice::Iter<'_, [u8; 4]>> = Default::default();
    /// assert_eq!(iter.count(), 0);
    /// ```
    fn default() -> Self {
        Flatten::new(Default::default())
    }
}

/// Real logic of both `Flatten` and `FlatMap` which simply delegate to
/// this type.
#[derive(Clone, Debug)]
#[unstable(feature = "trusted_len", issue = "37572")]
struct FlattenCompat<I, U> {
    iter: Fuse<I>,
    frontiter: Option<U>,
    backiter: Option<U>,
}
impl<I, U> FlattenCompat<I, U>
where
    I: Iterator,
{
    /// Adapts an iterator by flattening it, for use in `flatten()` and `flat_map()`.
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    const fn new(iter: I) -> FlattenCompat<I, U> {
        FlattenCompat { iter: iter.fuse(), frontiter: None, backiter: None }
    }
}

impl<I, U> FlattenCompat<I, U>
where
    I: Iterator<Item: IntoIterator<IntoIter = U>>,
{
    /// Folds the inner iterators into an accumulator by applying an operation.
    ///
    /// Folds over the inner iterators, not over their elements. Is used by the `fold`, `count`,
    /// and `last` methods.
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    #[inline]
    const fn iter_fold<Acc, Fold>(self, mut acc: Acc, mut fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, U) -> Acc,
        I: [const] Iterator<Item: [const] IntoIterator<IntoIter = U>>,
    {
        #[inline]
        const fn flatten<T: [const] IntoIterator, Acc>(
            fold: &mut impl [const] FnMut(Acc, T::IntoIter) -> Acc,
        ) -> impl [const] FnMut(Acc, T) -> Acc + '_ {
            move |acc, iter| fold(acc, iter.into_iter())
        }

        if let Some(iter) = self.frontiter {
            acc = fold(acc, iter);
        }

        acc = self.iter.fold(acc, flatten(&mut fold));

        if let Some(iter) = self.backiter {
            acc = fold(acc, iter);
        }

        acc
    }

    /// Folds over the inner iterators as long as the given function returns successfully,
    /// always storing the most recent inner iterator in `self.frontiter`.
    ///
    /// Folds over the inner iterators, not over their elements. Is used by the `try_fold` and
    /// `advance_by` methods.
    #[inline]
    const fn iter_try_fold<Acc, Fold, R>(&mut self, mut acc: Acc, mut fold: Fold) -> R
    where
        Fold: [const] FnMut(Acc, &mut U) -> R,
        R: [const] Try<Output = Acc>,
        I: [const] Iterator<Item: [const] IntoIterator<IntoIter = U>>,
    {
        #[inline]
        const fn flatten<'a, T: [const] IntoIterator, Acc, R: [const] Try<Output = Acc>>(
            frontiter: &'a mut Option<T::IntoIter>,
            fold: &'a mut impl [const] FnMut(Acc, &mut T::IntoIter) -> R,
        ) -> impl [const] FnMut(Acc, T) -> R + 'a {
            move |acc, iter| fold(acc, frontiter.insert(iter.into_iter()))
        }

        if let Some(iter) = &mut self.frontiter {
            acc = fold(acc, iter)?;
        }
        self.frontiter = None;

        acc = self.iter.try_fold(acc, flatten(&mut self.frontiter, &mut fold))?;
        self.frontiter = None;

        if let Some(iter) = &mut self.backiter {
            acc = fold(acc, iter)?;
        }
        self.backiter = None;

        try { acc }
    }
}

impl<I, U> FlattenCompat<I, U>
where
    I: DoubleEndedIterator<Item: IntoIterator<IntoIter = U>>,
{
    /// Folds the inner iterators into an accumulator by applying an operation, starting form the
    /// back.
    ///
    /// Folds over the inner iterators, not over their elements. Is used by the `rfold` method.
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    #[inline]
    const fn iter_rfold<Acc, Fold>(self, mut acc: Acc, mut fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, U) -> Acc,
        I: [const] DoubleEndedIterator<Item: [const] IntoIterator<IntoIter = U>>,
    {
        #[inline]
        const fn flatten<T: [const] IntoIterator, Acc>(
            fold: &mut impl [const] FnMut(Acc, T::IntoIter) -> Acc,
        ) -> impl [const] FnMut(Acc, T) -> Acc + '_ {
            move |acc, iter| fold(acc, iter.into_iter())
        }

        if let Some(iter) = self.backiter {
            acc = fold(acc, iter);
        }

        acc = self.iter.rfold(acc, flatten(&mut fold));

        if let Some(iter) = self.frontiter {
            acc = fold(acc, iter);
        }

        acc
    }

    /// Folds over the inner iterators in reverse order as long as the given function returns
    /// successfully, always storing the most recent inner iterator in `self.backiter`.
    ///
    /// Folds over the inner iterators, not over their elements. Is used by the `try_rfold` and
    /// `advance_back_by` methods.
    #[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
    #[inline]
    const fn iter_try_rfold<Acc, Fold, R>(&mut self, mut acc: Acc, mut fold: Fold) -> R
    where
        Fold: [const] FnMut(Acc, &mut U) -> R,
        R: [const] Try<Output = Acc>,
        I: [const] DoubleEndedIterator<Item: [const] IntoIterator<IntoIter = U>>,
    {
        #[inline]
        const fn flatten<'a, T: [const] IntoIterator, Acc, R: [const] Try>(
            backiter: &'a mut Option<T::IntoIter>,
            fold: &'a mut impl [const] FnMut(Acc, &mut T::IntoIter) -> R,
        ) -> impl [const] FnMut(Acc, T) -> R + 'a {
            move |acc, iter| fold(acc, backiter.insert(iter.into_iter()))
        }

        if let Some(iter) = &mut self.backiter {
            acc = fold(acc, iter)?;
        }
        self.backiter = None;

        acc = self.iter.try_rfold(acc, flatten(&mut self.backiter, &mut fold))?;
        self.backiter = None;

        if let Some(iter) = &mut self.frontiter {
            acc = fold(acc, iter)?;
        }
        self.frontiter = None;

        try { acc }
    }
}

// See also the `OneShot` specialization below.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const Iterator for FlattenCompat<I, U>
where
    I: [const] Iterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] Iterator,
{
    type Item = U::Item;

    #[inline]
    default fn next(&mut self) -> Option<U::Item> {
        loop {
            if let elt @ Some(_) = and_then_or_clear(&mut self.frontiter, Iterator::next) {
                return elt;
            }
            match self.iter.next() {
                None => return and_then_or_clear(&mut self.backiter, Iterator::next),
                Some(inner) => self.frontiter = Some(inner.into_iter()),
            }
        }
    }

    #[inline]
    default fn size_hint(&self) -> (usize, Option<usize>) {
        let (flo, fhi) = self.frontiter.as_ref().map_or((0, Some(0)), U::size_hint);
        let (blo, bhi) = self.backiter.as_ref().map_or((0, Some(0)), U::size_hint);
        let lo = flo.saturating_add(blo);

        if let Some(fixed_size) = <<I as Iterator>::Item as ConstSizeIntoIterator>::size() {
            let (lower, upper) = self.iter.size_hint();

            let lower = lower.saturating_mul(fixed_size).saturating_add(lo);
            let upper =
                try { fhi?.checked_add(bhi?)?.checked_add(fixed_size.checked_mul(upper?)?)? };

            return (lower, upper);
        }

        match (self.iter.size_hint(), fhi, bhi) {
            ((0, Some(0)), Some(a), Some(b)) => (lo, a.checked_add(b)),
            _ => (lo, None),
        }
    }

    #[inline]
    default fn try_fold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        #[inline]
        fn flatten<U: Iterator, Acc, R: [const] Try<Output = Acc>>(
            mut fold: impl FnMut(Acc, U::Item) -> R,
        ) -> impl FnMut(Acc, &mut U) -> R {
            move |acc, iter| iter.try_fold(acc, &mut fold)
        }

        self.iter_try_fold(init, flatten(fold))
    }

    #[inline]
    default fn fold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        #[inline]
        fn flatten<U: Iterator, Acc>(
            mut fold: impl FnMut(Acc, U::Item) -> Acc,
        ) -> impl FnMut(Acc, U) -> Acc {
            move |acc, iter| iter.fold(acc, &mut fold)
        }

        self.iter_fold(init, flatten(fold))
    }

    #[inline]
    #[rustc_inherit_overflow_checks]
    default fn advance_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        #[inline]
        #[rustc_inherit_overflow_checks]
        const fn advance<U: [const] Iterator>(n: usize, iter: &mut U) -> ControlFlow<(), usize> {
            match iter.advance_by(n) {
                Ok(()) => ControlFlow::Break(()),
                Err(remaining) => ControlFlow::Continue(remaining.get()),
            }
        }

        match self.iter_try_fold(n, advance) {
            ControlFlow::Continue(remaining) => NonZero::new(remaining).map_or(Ok(()), Err),
            _ => Ok(()),
        }
    }

    #[inline]
    default fn count(self) -> usize {
        #[inline]
        #[rustc_inherit_overflow_checks]
        const fn count<U: [const] Iterator>(acc: usize, iter: U) -> usize {
            acc + iter.count()
        }

        self.iter_fold(0, count)
    }

    #[inline]
    default fn last(self) -> Option<Self::Item> {
        #[inline]
        const fn last<U: [const] Iterator>(last: Option<U::Item>, iter: U) -> Option<U::Item> {
            iter.last().or(last)
        }

        self.iter_fold(None, last)
    }
}

// See also the `OneShot` specialization below.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const DoubleEndedIterator for FlattenCompat<I, U>
where
    I: [const] DoubleEndedIterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] DoubleEndedIterator,
{
    #[inline]
    default fn next_back(&mut self) -> Option<U::Item> {
        loop {
            if let elt @ Some(_) = and_then_or_clear(&mut self.backiter, |b| b.next_back()) {
                return elt;
            }
            match self.iter.next_back() {
                None => return and_then_or_clear(&mut self.frontiter, |f| f.next_back()),
                Some(inner) => self.backiter = Some(inner.into_iter()),
            }
        }
    }

    #[inline]
    default fn try_rfold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        #[inline]
        const fn flatten<U: [const] DoubleEndedIterator, Acc, R: [const] Try<Output = Acc>>(
            mut fold: impl [const] FnMut(Acc, U::Item) -> R,
        ) -> impl [const] FnMut(Acc, &mut U) -> R {
            move |acc, iter| iter.try_rfold(acc, &mut fold)
        }

        self.iter_try_rfold(init, flatten(fold))
    }

    #[inline]
    default fn rfold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        #[inline]
        const fn flatten<U: [const] DoubleEndedIterator, Acc>(
            mut fold: impl [const] FnMut(Acc, U::Item) -> Acc,
        ) -> impl [const] FnMut(Acc, U) -> Acc {
            move |acc, iter| iter.rfold(acc, &mut fold)
        }

        self.iter_rfold(init, flatten(fold))
    }

    #[inline]
    #[rustc_inherit_overflow_checks]
    default fn advance_back_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        #[inline]
        #[rustc_inherit_overflow_checks]
        const fn advance<U: [const] DoubleEndedIterator>(
            n: usize,
            iter: &mut U,
        ) -> ControlFlow<(), usize> {
            match iter.advance_back_by(n) {
                Ok(()) => ControlFlow::Break(()),
                Err(remaining) => ControlFlow::Continue(remaining.get()),
            }
        }

        match self.iter_try_rfold(n, advance) {
            ControlFlow::Continue(remaining) => NonZero::new(remaining).map_or(Ok(()), Err),
            _ => Ok(()),
        }
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<const N: usize, I, T> const TrustedLen
    for FlattenCompat<I, <[T; N] as IntoIterator>::IntoIter>
where
    I: [const] TrustedLen<Item = [T; N]>,
{
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<'a, const N: usize, I, T> const TrustedLen
    for FlattenCompat<I, <&'a [T; N] as IntoIterator>::IntoIter>
where
    I: [const] TrustedLen<Item = &'a [T; N]>,
{
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
unsafe impl<'a, const N: usize, I, T> const TrustedLen
    for FlattenCompat<I, <&'a mut [T; N] as IntoIterator>::IntoIter>
where
    I: [const] TrustedLen<Item = &'a mut [T; N]>,
{
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
const trait ConstSizeIntoIterator: [const] IntoIterator {
    // FIXME(#31844): convert to an associated const once specialization supports that
    fn size() -> Option<usize>;
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const ConstSizeIntoIterator for T
where
    T: [const] IntoIterator,
{
    #[inline]
    default fn size() -> Option<usize> {
        None
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T, const N: usize> const ConstSizeIntoIterator for [T; N] {
    #[inline]
    fn size() -> Option<usize> {
        Some(N)
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T, const N: usize> const ConstSizeIntoIterator for &[T; N] {
    #[inline]
    fn size() -> Option<usize> {
        Some(N)
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T, const N: usize> const ConstSizeIntoIterator for &mut [T; N] {
    #[inline]
    fn size() -> Option<usize> {
        Some(N)
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
#[inline]
const fn and_then_or_clear<T, U>(
    opt: &mut Option<T>,
    f: impl [const] FnOnce(&mut T) -> Option<U>,
) -> Option<U> {
    let x = f(opt.as_mut()?);
    if x.is_none() {
        *opt = None;
    }
    x
}

/// Specialization trait for iterator types that never return more than one item.
///
/// Note that we still have to deal with the possibility that the iterator was
/// already exhausted before it came into our control.
#[rustc_specialization_trait]
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
const trait OneShot {}

// These all have exactly one item, if not already consumed.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for Once<T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<F> const OneShot for OnceWith<F> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for array::IntoIter<T, 1> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for option::IntoIter<T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for option::Iter<'_, T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for option::IterMut<'_, T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for result::IntoIter<T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for result::Iter<'_, T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for result::IterMut<'_, T> {}

// These are always empty, which is fine to optimize too.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for Empty<T> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<T> const OneShot for array::IntoIter<T, 0> {}

// These adapters never increase the number of items.
// (There are more possible, but for now this matches BoundedSize above.)
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot> const OneShot for Cloned<I> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot> const OneShot for Copied<I> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot, P> const OneShot for Filter<I, P> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot, P> const OneShot for FilterMap<I, P> {}
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot, F> const OneShot for Map<I, F> {}

// Blanket impls pass this property through as well
// (but we can't do `Box<I>` unless we expose this trait to alloc)
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I: [const] OneShot> const OneShot for &mut I {}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
#[inline]
const fn into_item<I>(inner: I) -> Option<I::Item>
where
    I: [const] IntoIterator<IntoIter: [const] OneShot>,
{
    inner.into_iter().next()
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
#[inline]
const fn flatten_one<I: [const] IntoIterator<IntoIter: [const] OneShot>, Acc>(
    mut fold: impl [const] FnMut(Acc, I::Item) -> Acc,
) -> impl [const] FnMut(Acc, I) -> Acc {
    move |acc, inner| match inner.into_iter().next() {
        Some(item) => fold(acc, item),
        None => acc,
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
#[inline]
const fn try_flatten_one<
    I: [const] IntoIterator<IntoIter: [const] OneShot>,
    Acc,
    R: [const] Try<Output = Acc>,
>(
    mut fold: impl [const] FnMut(Acc, I::Item) -> R,
) -> impl [const] FnMut(Acc, I) -> R {
    move |acc, inner| match inner.into_iter().next() {
        Some(item) => fold(acc, item),
        None => try { acc },
    }
}

#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
#[inline]
const fn advance_by_one<I>(n: NonZero<usize>, inner: I) -> Option<NonZero<usize>>
where
    I: [const] IntoIterator<IntoIter: [const] OneShot>,
{
    match inner.into_iter().next() {
        Some(_) => NonZero::new(n.get() - 1),
        None => Some(n),
    }
}

// Specialization: When the inner iterator `U` never returns more than one item, the `frontiter` and
// `backiter` states are a waste, because they'll always have already consumed their item. So in
// this impl, we completely ignore them and just focus on `self.iter`, and we only call the inner
// `U::next()` one time.
//
// It's mostly fine if we accidentally mix this with the more generic impls, e.g. by forgetting to
// specialize one of the methods. If the other impl did set the front or back, we wouldn't see it
// here, but it would be empty anyway; and if the other impl looked for a front or back that we
// didn't bother setting, it would just see `None` (or a previous empty) and move on.
//
// An exception to that is `advance_by(0)` and `advance_back_by(0)`, where the generic impls may set
// `frontiter` or `backiter` without consuming the item, so we **must** override those.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const Iterator for FlattenCompat<I, U>
where
    I: [const] Iterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] Iterator + [const] OneShot,
{
    #[inline]
    fn next(&mut self) -> Option<U::Item> {
        while let Some(inner) = self.iter.next() {
            if let item @ Some(_) = inner.into_iter().next() {
                return item;
            }
        }
        None
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.iter.size_hint();
        match <I::Item as ConstSizeIntoIterator>::size() {
            Some(0) => (0, Some(0)),
            Some(1) => (lower, upper),
            _ => (0, upper),
        }
    }

    #[inline]
    fn try_fold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.iter.try_fold(init, try_flatten_one(fold))
    }

    #[inline]
    fn fold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.iter.fold(init, flatten_one(fold))
    }

    #[inline]
    fn advance_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        if let Some(n) = NonZero::new(n) {
            self.iter.try_fold(n, advance_by_one).map_or(Ok(()), Err)
        } else {
            // Just advance the outer iterator
            self.iter.advance_by(0)
        }
    }

    #[inline]
    fn count(self) -> usize {
        self.iter.filter_map(into_item).count()
    }

    #[inline]
    fn last(self) -> Option<Self::Item> {
        self.iter.filter_map(into_item).last()
    }
}

// Note: We don't actually care about `U: [const] DoubleEndedIterator`, since forward and backward are the
// same for a one-shot iterator, but we have to keep that to match the default specialization.
#[rustc_const_unstable(feature = "const_cmp", issue = "143800")]
impl<I, U> const DoubleEndedIterator for FlattenCompat<I, U>
where
    I: [const] DoubleEndedIterator<Item: [const] IntoIterator<IntoIter = U, Item = U::Item>>,
    U: [const] DoubleEndedIterator + [const] OneShot,
{
    #[inline]
    fn next_back(&mut self) -> Option<U::Item> {
        while let Some(inner) = self.iter.next_back() {
            if let item @ Some(_) = inner.into_iter().next() {
                return item;
            }
        }
        None
    }

    #[inline]
    fn try_rfold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: [const] FnMut(Acc, Self::Item) -> R,
        R: [const] Try<Output = Acc>,
    {
        self.iter.try_rfold(init, try_flatten_one(fold))
    }

    #[inline]
    fn rfold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: [const] FnMut(Acc, Self::Item) -> Acc,
    {
        self.iter.rfold(init, flatten_one(fold))
    }

    #[inline]
    fn advance_back_by(&mut self, n: usize) -> Result<(), NonZero<usize>> {
        if let Some(n) = NonZero::new(n) {
            self.iter.try_rfold(n, advance_by_one).map_or(Ok(()), Err)
        } else {
            // Just advance the outer iterator
            self.iter.advance_back_by(0)
        }
    }
}
