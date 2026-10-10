/// Hard upper bound on disk-bound media downloads ([`crate::media::download_to`]).
///
/// #332: same upstream-trust boundary as the voice path (`file.size`
/// is reported by Telegram, not verified), different downstream
/// threat — RAM-OOM on the voice path, **disk-fill** here. Telegram's
/// default bot-API limit is 50 MB for documents; 50 MiB matches that
/// and stays an order of magnitude below typical operator disk
/// budgets, while comfortably covering photo + document payloads
/// teamctl operators actually exchange. Bump requires a release-notes
/// entry + this constant — #332.
pub(super) const MAX_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;

/// Operator-facing error when Telegram already reports a media file
/// over the ceiling — fast-fail before opening any sink. The `kind`
/// label distinguishes voice (RAM-OOM defense, #279) from media
/// (disk-fill defense, #332) so operators can tell which boundary
/// fired without log-diving. Pure for unit-testability.
pub(super) fn media_size_pre_reject(kind: &str, reported: u32, max: usize) -> String {
    format!("{kind} too large: {reported} bytes (max {max})")
}

/// Operator-facing error when cumulative bytes mid-download would pass
/// the ceiling — fires from [`BoundedWriter`] when `file.size` lied
/// small but the actual stream tries to flood us. The `kind` label
/// threads from the wrapper's construction so the message names the
/// right boundary (`voice file` vs `media file`).
pub(super) fn media_size_mid_reject(
    kind: &str,
    sofar: usize,
    incoming: usize,
    max: usize,
) -> String {
    let proposed = sofar.saturating_add(incoming);
    format!(
        "{kind} exceeded ceiling mid-download \
         (max {max} bytes; upstream sent at least {proposed})"
    )
}

/// `tokio::io::AsyncWrite` adapter that aborts with
/// `io::ErrorKind::InvalidData` once cumulative bytes would exceed
/// `max`. Wraps the voice-download `Vec<u8>` (#279) AND the disk-write
/// `tokio::fs::File` handle in [`crate::media::download_to`] (#332) so a lying
/// upstream can't stream past the ceiling even when Telegram's
/// reported `file.size` looked small. Generic over `W: AsyncWrite +
/// Unpin + Send`, so the same wrapper works at both sinks; the `kind`
/// label threads into mid-download error messages so operators can
/// tell which boundary fired.
pub(super) struct BoundedWriter<W> {
    pub(super) kind: &'static str,
    pub(super) inner: W,
    pub(super) max: usize,
    pub(super) written: usize,
}

impl<W> BoundedWriter<W> {
    pub(super) fn new(kind: &'static str, inner: W, max: usize) -> Self {
        Self {
            kind,
            inner,
            max,
            written: 0,
        }
    }
    pub(super) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for BoundedWriter<W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.written.saturating_add(buf.len()) > this.max {
            let msg = media_size_mid_reject(this.kind, this.written, buf.len(), this.max);
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                msg,
            )));
        }
        match std::pin::Pin::new(&mut this.inner).poll_write(cx, buf) {
            std::task::Poll::Ready(Ok(n)) => {
                this.written = this.written.saturating_add(n);
                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
