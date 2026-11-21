import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { dashedSeparator, extractOriginalSender, htmlToText, pickBody } from '../src/body.js';
import { MAX_BODY_BYTES, MAX_SUBJECT_CHARS, MAX_TITLE_CHARS } from '../src/limits.js';
import { cleanSubject, EMPTY_TITLE } from '../src/subject.js';
import { charCount, truncateUtf8, utf8Length } from '../src/text.js';

describe('cleanSubject', () => {
  it('removes FW, Fwd, WG and Re chains, keeping the original', () => {
    const cases = [
      ['FW: Report', 'Report'],
      ['Fwd: Re: FW: Report', 'Report'],
      ['WG: AW: Termin', 'Termin'],
      ['re: RE[2]: Re(3): fw : Hello', 'Hello'],
      ['Fwd：Full-width colon', 'Full-width colon'],
      ['Report: Q4 numbers', 'Report: Q4 numbers'],
      ['Refund request', 'Refund request'],
      ['Fwd: Re: Fw: ', EMPTY_TITLE],
    ];
    for (const [input, title] of cases) {
      const r = cleanSubject(input);
      assert.equal(r.title, title, input);
      assert.equal(r.original, input.trim().replace(/\s+/g, ' '));
      assert.equal(r.keep, false);
    }
  });

  it('detects and removes #keep anywhere, case-insensitively', () => {
    assert.deepEqual(cleanSubject('Fwd: #keep Invoice'), { original: 'Fwd: #keep Invoice', title: 'Invoice', keep: true });
    assert.deepEqual(cleanSubject('FW: Invoice #KEEP'), { original: 'FW: Invoice #KEEP', title: 'Invoice', keep: true });
    assert.equal(cleanSubject('#keep').title, EMPTY_TITLE);
    assert.equal(cleanSubject('Invoice (#keep)').keep, false);
    assert.equal(cleanSubject('Invoice #keeper').keep, false);
    assert.deepEqual(cleanSubject('Invoice #keep.'), { original: 'Invoice #keep.', title: 'Invoice', keep: true });
    assert.equal(cleanSubject('x#keep').keep, false);
  });

  it('strips forward prefixes before looking for #keep', () => {
    assert.deepEqual(cleanSubject('Fwd:#keep'), { original: 'Fwd:#keep', title: EMPTY_TITLE, keep: true });
    assert.deepEqual(cleanSubject('Fwd:#keep Invoice'), { original: 'Fwd:#keep Invoice', title: 'Invoice', keep: true });
    assert.deepEqual(cleanSubject('RE:FW:#KEEP Re: Invoice'), { original: 'RE:FW:#KEEP Re: Invoice', title: 'Invoice', keep: true });
  });

  it('never creates #keep by truncating a longer hashtag', () => {
    const s = `${'x'.repeat(MAX_SUBJECT_CHARS - 6)} #keeper`;
    const r = cleanSubject(s);
    assert.ok(r.original.endsWith('#keep'), 'the capped original happens to end in #keep');
    assert.equal(r.keep, false);
    const t = cleanSubject(`${'y'.repeat(MAX_TITLE_CHARS - 6)} #keeper and more`);
    assert.equal(t.keep, false);
  });

  it('handles missing subjects and control characters', () => {
    assert.equal(cleanSubject(undefined).title, EMPTY_TITLE);
    assert.equal(cleanSubject('A\u0000B\tC\r\nD').title, 'AB C D');
  });

  it('caps the original subject and the title by code points', () => {
    const long = `Fwd: ${'ž'.repeat(5000)}`;
    const r = cleanSubject(long);
    assert.equal(charCount(r.original), MAX_SUBJECT_CHARS);
    assert.equal(charCount(r.title), MAX_TITLE_CHARS);
    const emoji = cleanSubject('😀'.repeat(600));
    assert.equal(charCount(emoji.title), MAX_TITLE_CHARS);
    assert.ok(!/[\ud800-\udbff]$/.test(emoji.title), 'no lone surrogate at the cut');
  });
});

describe('htmlToText', () => {
  it('strips tags, scripts and styles, and keeps structure', () => {
    const html =
      '<html><head><title>T</title><style>p{color:red}</style></head><body>' +
      '<p>Hello&nbsp;<b>world</b> &amp; friends &#x2713; &#269;</p><script>alert(1)</script>' +
      '<!-- hidden --><ul><li>one</li><li>two</li></ul>line<br>break<div>end &unknown;</div></body></html>';
    assert.equal(htmlToText(html), 'Hello world & friends ✓ č\n\n- one\n- two\nline\nbreak\nend &unknown;');
  });

  it('keeps stray angle brackets as text and drops doctype', () => {
    assert.equal(htmlToText('<!DOCTYPE html>a < b and c<d <p>x</p>'), 'a < b and c<d\nx');
  });

  it('ignores > inside quoted attribute values', () => {
    assert.equal(htmlToText('<a title="x>leak" href=\'y>leak\'>link</a> after'), 'link after');
    assert.equal(htmlToText('<img alt = "a > b">text'), 'text');
    assert.equal(htmlToText("it's <b>bold</b>"), "it's bold");
    assert.ok(htmlToText('<a href="unclosed>text').includes('text'), 'an unclosed quote degrades to text');
  });

  it('stays linear on hostile markup', () => {
    const hostile = [
      '<'.repeat(200_000),
      '<!--'.repeat(50_000),
      '<style>'.repeat(30_000),
      `<a${' x'.repeat(100_000)}`,
      '<a x="'.repeat(40_000),
      `<a x="${'y'.repeat(200_000)}`,
    ];
    for (const bad of hostile) {
      const t0 = performance.now();
      htmlToText(bad);
      assert.ok(performance.now() - t0 < 500, `took too long on ${bad.slice(0, 10)}`);
    }
  });

  it('drops invalid character references', () => {
    assert.equal(htmlToText('a&#0;b&#xD800;c&#x110000;d'), 'abcd');
  });
});

describe('pickBody', () => {
  it('prefers text/plain', () => {
    assert.deepEqual(pickBody({ text: 'plain', html: '<p>html</p>' }), { text: 'plain', source: 'text', truncated: false });
  });

  it('falls back to stripped HTML', () => {
    assert.deepEqual(pickBody({ text: '  ', html: '<p>html</p>' }), { text: 'html', source: 'html', truncated: false });
  });

  it('returns null without content', () => {
    assert.equal(pickBody({}), null);
  });

  it('truncates to 64 KiB on a character boundary', () => {
    const text = `a${'č'.repeat(MAX_BODY_BYTES)}`; // 1 + 2 bytes each: the cut lands mid-character
    const b = pickBody({ text });
    assert.ok(b);
    assert.equal(b.truncated, true);
    assert.ok(utf8Length(b.text) <= MAX_BODY_BYTES);
    assert.ok(utf8Length(b.text) >= MAX_BODY_BYTES - 1);
    assert.ok(!b.text.includes('�'));
  });

  it('truncateUtf8 leaves short strings alone', () => {
    assert.deepEqual(truncateUtf8('abc', 3), { text: 'abc', truncated: false });
    assert.deepEqual(truncateUtf8('ab😀', 5), { text: 'ab', truncated: true });
  });
});

describe('extractOriginalSender', () => {
  it('reads a Gmail-style forwarded header block', () => {
    const t = 'FYI\n\n---------- Forwarded message ---------\nFrom: Alice Example <Alice@Example.org>\nDate: today\n';
    assert.deepEqual(extractOriginalSender(t), { address: 'alice@example.org', name: 'Alice Example' });
  });

  it('reads an Outlook-style block with mailto', () => {
    const t = '-----Original Message-----\nFrom: Bob Example [mailto:bob@example.net]\nSent: Monday\n';
    assert.deepEqual(extractOriginalSender(t), { address: 'bob@example.net', name: 'Bob Example' });
  });

  it('reads localized and bold labels, and bare addresses', () => {
    assert.deepEqual(extractOriginalSender('________________________________\nOd: "Nová, Eva" <eva@example.org>\nOdesláno: pondělí\n'), {
      address: 'eva@example.org',
      name: 'Nová, Eva',
    });
    assert.deepEqual(extractOriginalSender('*From:* carol@example.org\n*Sent:* Monday\n'), { address: 'carol@example.org', name: null });
    assert.deepEqual(extractOriginalSender('Von: Dieter <dieter@example.net>\nBetreff: Termin'), { address: 'dieter@example.net', name: 'Dieter' });
    assert.deepEqual(extractOriginalSender('> Date: Monday\n> From: Quoted <q@example.org>'), { address: 'q@example.org', name: 'Quoted' });
    assert.deepEqual(extractOriginalSender('Begin forwarded message:\n\nFrom: Apple <a@example.org>'), { address: 'a@example.org', name: 'Apple' });
  });

  it('requires evidence of a forwarded header block', () => {
    assert.equal(extractOriginalSender('From: someone@example.org'), null);
    assert.equal(extractOriginalSender('Thanks!\nFrom: Bob <bob@example.net>\nBest regards'), null);
    assert.equal(extractOriginalSender('From: a@example.org\nFrom: b@example.org'), null, 'two From lines are not a block');
    assert.equal(extractOriginalSender('Note from: me@example.org\nTo: x'), null, 'not a header label');
  });

  it('skips From lines without an address and returns null when there is none', () => {
    const t = 'From: Alice Example\nSent: Monday\n\n-----Original Message-----\nFrom: Bob <bob@example.net>\nTo: you';
    assert.deepEqual(extractOriginalSender(t), { address: 'bob@example.net', name: 'Bob' });
    assert.equal(extractOriginalSender('No header block here.\nfrom the team'), null);
    assert.equal(extractOriginalSender(undefined), null);
  });

  it('stays linear on adversarial From lines', () => {
    const cases = [
      // The old regex backtracked on these: ~330 ms for one line.
      `From: ${'a'.repeat(990)}\nSent: x`,
      `From: ${'a.'.repeat(495)}\nSent: x`,
      `From: ${'@'.repeat(990)}\nSent: x`,
      `From: ${'<'.repeat(990)}\nSent: x`,
      `From: ${'a'.repeat(200_000)}\nSent: x`,
      Array.from({ length: 3000 }, () => `From: ${'b'.repeat(15)}`).join('\nSent: y\n'),
    ];
    for (const t of cases) {
      const t0 = performance.now();
      extractOriginalSender(t);
      assert.ok(performance.now() - t0 < 50, `took ${(performance.now() - t0).toFixed(1)} ms on ${t.slice(0, 12)}`);
    }
  });

  it('stays linear on adversarial separator lines before a From line', () => {
    // The old /^-{5,}[^-]{0,60}-{2,}$/ backtracked quadratically here: ~30 ms for this text.
    const dashes = `${'-'.repeat(990)}x`;
    const t = Array.from({ length: 40 }, () => [dashes, dashes, dashes, 'From: a'].join('\n')).join('\n');
    const t0 = performance.now();
    extractOriginalSender(t);
    assert.ok(performance.now() - t0 < 10, `took ${(performance.now() - t0).toFixed(1)} ms`);
  });

  it('recognizes dashed separators exactly as before', () => {
    const yes = ['-----Original Message-----', '---------- Forwarded message ---------', '-------', `-----${'x'.repeat(60)}--`];
    const no = ['----Original Message-----', '-----Original Message-', '------', `-----${'x'.repeat(61)}--`, '-----a-b-----', 'x-----y--', ''];
    for (const s of yes) assert.ok(dashedSeparator(s), s);
    for (const s of no) assert.ok(!dashedSeparator(s), s);
    const old = /^-{5,}[^-]{0,60}-{2,}$/;
    for (const s of [...yes, ...no, '-----', '-----x', '--x--', '-------x--', '-----  --']) assert.equal(dashedSeparator(s), old.test(s), s);
  });

  it('skips lines longer than the RFC 5322 limit', () => {
    assert.equal(extractOriginalSender(`From: ${'x'.repeat(1000)} <long@example.org>\nSent: x`), null);
  });

  it('only scans the start of very long text', () => {
    const t = `${'x'.repeat(70 * 1024)}\nFrom: Late <late@example.org>`;
    assert.equal(extractOriginalSender(t), null);
  });
});
