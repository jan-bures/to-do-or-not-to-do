// Every bound the Worker enforces, in one place. See docs/ingest-email.md for why each value was chosen.

/**
 * Bytes of raw MIME (headers and body) the Worker reads and parses; the rest of the stream is never
 * read. Headers and the text parts come first in a forwarded message, so this covers everything we
 * use. Parsing 256 KiB takes about 5 ms of CPU, while 1 MiB took about 17 ms when measured. The
 * Workers Free plan allows 10 ms per invocation.
 */
export const MAX_RAW_BYTES = 256 * 1024;

/** Longest original subject kept, in characters (RFC 5322 line length). */
export const MAX_SUBJECT_CHARS = 998;

/** Longest task title, in characters. Matches `todo_core::decide::MAX_TITLE`. */
export const MAX_TITLE_CHARS = 500;

/** Longest kept body (`#keep`), in UTF-8 bytes. */
export const MAX_BODY_BYTES = 64 * 1024;

/** Longest display name kept for the original sender, in characters. */
export const MAX_NAME_CHARS = 200;

/** Longest email address accepted, in characters (RFC 5321 path limit). */
export const MAX_ADDRESS_CHARS = 320;

/** Longest `Message-ID` accepted, in characters. Longer ones are replaced by a synthesized ID. */
export const MAX_MESSAGE_ID_CHARS = 998;

/** Characters of decoded text scanned for a forwarded header block. */
export const MAX_SCAN_CHARS = 64 * 1024;

/** Lines of decoded text scanned for a forwarded header block. */
export const MAX_SCAN_LINES = 2000;

/** Longest line considered as a forwarded header line, in characters (RFC 5322 line limit). */
export const MAX_LINE_CHARS = 998;

/** Longest HTML tag the HTML-to-text scan accepts, in characters. Longer ones are kept as text. */
export const MAX_TAG_CHARS = 4096;

/** Shortest accepted RECIPIENT_SECRET, in characters. */
export const MIN_RECIPIENT_SECRET_CHARS = 16;

/**
 * Default for MAX_ACCEPTED_PER_DAY: accepted emails per isolate in a sliding 24 h window. Sized from
 * the Queues Free plan (10,000 operations per day): an email that is retried for the whole window
 * costs at most about 120 operations (see docs/ingest-email.md), so 50 such emails stay near 6,000.
 */
export const DEFAULT_MAX_ACCEPTED_PER_DAY = 50;

/**
 * Largest serialized request body, in UTF-8 bytes. Queue messages are limited to 128 KB
 * (1 KB = 1000 bytes, including about 100 bytes of metadata), and the queued envelope adds a
 * few dozen bytes around the payload, so this leaves ample headroom.
 */
export const MAX_PAYLOAD_BYTES = 100_000;

/** Largest queued envelope, in UTF-8 bytes. Checked before every `send`. */
export const MAX_QUEUE_MESSAGE_BYTES = 120_000;

/**
 * How long after acceptance the Worker keeps retrying a delivery. 23 h rather than 24 h: if
 * re-enqueueing ever fails and a retry falls back to msg.retry(), that message keeps its original
 * write time, and the Free plan deletes messages 24 h after it. The spare hour leaves room for the
 * final notice and its retries.
 */
export const RETRY_WINDOW_MS = 23 * 60 * 60 * 1000;

/** Delay before retry n (1-based), in seconds. Later retries use `MAX_RETRY_DELAY_S`. */
export const RETRY_DELAYS_S = [30, 60, 120, 300, 600, 1200, 1800, 3600];

/** Longest delay between retries, in seconds. Queues allow up to 24 h. */
export const MAX_RETRY_DELAY_S = 2 * 60 * 60;

/** Hard stop on the number of retries, whatever the clock says. */
export const MAX_RETRIES = 40;

/** Timeout of one POST to the server, in milliseconds. */
export const POST_TIMEOUT_MS = 15_000;

/** Minimum length of `INGEST_HMAC_SECRET`, in characters. */
export const MIN_SECRET_CHARS = 32;

/**
 * Limits passed to postal-mime, far below its defaults. Parse time grows with the number of parts:
 * at 256 KiB, 300 parts took about 6 ms (13 ms cold), 100 parts about 2 ms. A message over a limit
 * still becomes a task, built from its headers only (`parse_failed`).
 */
export const PARSER_LIMITS = Object.freeze({
  maxNestingDepth: 16,
  maxHeadersSize: 256 * 1024,
  maxPartCount: 100,
  maxRfc822NestingDepth: 3,
  maxForwardedAddresses: 500,
});
