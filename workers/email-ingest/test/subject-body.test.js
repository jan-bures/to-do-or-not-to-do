import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { extractOriginalSender, htmlToText, pickBody } from '../src/body.js';
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
    assert.equal(cleanSubject('Invoice #keep.').title, 'Invoice .');
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

  it('stays linear on hostile markup', () => {
    for (const bad of ['<'.repeat(200_000), '<!--'.repeat(50_000), '<style>'.repeat(30_000), `<a${' x'.repeat(100_000)}`]) {
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
    assert.deepEqual(extractOriginalSender('________________________________\nOd: "Nová, Eva" <eva@example.cz>\n'), {
      address: 'eva@example.cz',
      name: 'Nová, Eva',
    });
    assert.deepEqual(extractOriginalSender('*From:* carol@example.org\n'), { address: 'carol@example.org', name: null });
    assert.deepEqual(extractOriginalSender('Von: Dieter <dieter@example.de>'), { address: 'dieter@example.de', name: 'Dieter' });
    assert.deepEqual(extractOriginalSender('> From: Quoted <q@example.org>'), { address: 'q@example.org', name: 'Quoted' });
  });

  it('skips From lines without an address and returns null when there is none', () => {
    assert.deepEqual(extractOriginalSender('From: Alice Example\nFrom: Bob <bob@example.net>'), { address: 'bob@example.net', name: 'Bob' });
    assert.equal(extractOriginalSender('No header block here.\nfrom the team'), null);
    assert.equal(extractOriginalSender(undefined), null);
  });

  it('only scans the start of very long text', () => {
    const t = `${'x'.repeat(70 * 1024)}\nFrom: Late <late@example.org>`;
    assert.equal(extractOriginalSender(t), null);
  });
});
