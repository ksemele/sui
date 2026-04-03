// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use bytes::Buf;
use http_body::Body;
use http_body::Frame;
use http_body::SizeHint;
use pin_project_lite::pin_project;
use tower::Layer;
use tower::Service;

/// The number of bytes received in the response body.
///
/// This is inserted into the `http::Extensions` of the `tonic::Response` and can be retrieved via:
///
/// ```ignore
/// let response = client.some_rpc(request).await?;
/// if let Some(count) = response.extensions().get::<ByteCount>() {
///     println!("received {} bytes", count.value());
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ByteCount(Arc<AtomicUsize>);

impl ByteCount {
    /// Returns the number of bytes received in the response body.
    pub fn value(&self) -> usize {
        // Relaxed is sufficient because all writes (via poll_frame) complete before the
        // response is returned to the caller, so there is no concurrent access.
        // AtomicUsize is only needed to satisfy the Send + Sync bounds of http::Extensions.
        self.0.load(Ordering::Relaxed)
    }
}

/// A [`Layer`] that counts the number of bytes in the response body.
///
/// The byte count is made available via [`ByteCount`] in the response's extensions.
#[derive(Clone, Debug, Default)]
pub struct ByteCountLayer;

impl<S> Layer<S> for ByteCountLayer {
    type Service = ByteCountService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ByteCountService { inner }
    }
}

/// A [`Service`] wrapper that counts the number of bytes in the response body.
#[derive(Clone, Debug)]
pub struct ByteCountService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody, E> Service<http::Request<ReqBody>> for ByteCountService<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>, Error = E>,
{
    type Response = http::Response<ByteCountBody<ResBody>>;
    type Error = E;
    type Future = ByteCountFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<ReqBody>) -> Self::Future {
        ByteCountFuture {
            inner: self.inner.call(request),
        }
    }
}

pin_project! {
    #[doc(hidden)]
    pub struct ByteCountFuture<F> {
        #[pin]
        inner: F,
    }
}

impl<F, B, E> Future for ByteCountFuture<F>
where
    F: Future<Output = Result<http::Response<B>, E>>,
{
    type Output = Result<http::Response<ByteCountBody<B>>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.inner.poll(cx).map(|result| {
            result.map(|response| {
                let (mut parts, body) = response.into_parts();
                let count = Arc::new(AtomicUsize::new(0));
                parts.extensions.insert(ByteCount(count.clone()));
                http::Response::from_parts(parts, ByteCountBody { inner: body, count })
            })
        })
    }
}

pin_project! {
    /// A body wrapper that counts the number of bytes read from the inner body.
    pub struct ByteCountBody<B> {
        #[pin]
        inner: B,
        count: Arc<AtomicUsize>,
    }
}

impl<B> Body for ByteCountBody<B>
where
    B: Body,
    B::Data: Buf,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        match this.inner.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.count.fetch_add(data.remaining(), Ordering::Relaxed);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::convert::Infallible;

    use bytes::Bytes;
    use tower::ServiceExt;

    struct MultiFrameBody(VecDeque<Bytes>);

    impl MultiFrameBody {
        fn new(frames: impl IntoIterator<Item = &'static str>) -> Self {
            Self(frames.into_iter().map(Bytes::from).collect())
        }
    }

    impl Body for MultiFrameBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(self.0.pop_front().map(|data| Ok(Frame::data(data))))
        }
    }

    struct MockService(Option<MultiFrameBody>);

    impl Service<http::Request<()>> for MockService {
        type Response = http::Response<MultiFrameBody>;
        type Error = Infallible;
        type Future = std::future::Ready<Result<http::Response<MultiFrameBody>, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: http::Request<()>) -> Self::Future {
            std::future::ready(Ok(http::Response::new(self.0.take().unwrap())))
        }
    }

    async fn run_byte_count(frames: &[&'static str]) -> usize {
        let body = MultiFrameBody::new(frames.iter().copied());
        let mut svc = ByteCountLayer.layer(MockService(Some(body)));

        let response = svc
            .ready()
            .await
            .unwrap()
            .call(http::Request::new(()))
            .await
            .unwrap();
        let byte_count = response.extensions().get::<ByteCount>().cloned().unwrap();

        let mut body = response.into_body();

        // Drain the body to compute the count.
        let waker = futures::task::noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        while let Poll::Ready(Some(Ok(_))) = Pin::new(&mut body).poll_frame(&mut cx) {}

        byte_count.value()
    }

    #[tokio::test]
    async fn test_byte_count_0_frames() {
        assert_eq!(run_byte_count(&[]).await, 0);
    }

    #[tokio::test]
    async fn test_byte_count_1_frame() {
        assert_eq!(run_byte_count(&["a"]).await, 1);
    }

    #[tokio::test]
    async fn test_byte_count_2_frames() {
        assert_eq!(run_byte_count(&["a", "bb"]).await, 3);
    }
}
