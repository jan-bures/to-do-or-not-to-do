import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { domainOf, parseAllowlist, parseRecipient, parseSingleMailbox } from '../src/address.js';

describe('parseRecipient', () => {
  it('plain todo@ carries no list', () => {
    assert.deepEqual(parseRecipient('todo@in.example.com', 'todo'), { ok: true, list: null });
  });

  it('todo+<list>@ carries the list, lowercased', () => {
    assert.deepEqual(parseRecipient('todo+shopping@in.example.com', 'todo'), { ok: true, list: 'shopping' });
    assert.deepEqual(parseRecipient('<TODO+Work_2025-q4@in.example.com>', 'todo'), { ok: true, list: 'work_2025-q4' });
  });

  it('rejects an empty list suffix', () => {
    assert.deepEqual(parseRecipient('todo+@in.example.com', 'todo'), { ok: false, reason: 'bad_list' });
  });

  it('only the first + splits', () => {
    assert.deepEqual(parseRecipient('todo+a+b@in.example.com', 'todo'), { ok: false, reason: 'bad_list' });
  });

  it('rejects invalid list keys', () => {
    for (const to of ['todo+-x@in.example.com', 'todo+a.b@in.example.com', `todo+${'a'.repeat(65)}@in.example.com`, 'todo+ž@in.example.com']) {
      assert.deepEqual(parseRecipient(to, 'todo'), { ok: false, reason: 'bad_list' }, to);
    }
  });

  it('rejects other local parts', () => {
    assert.deepEqual(parseRecipient('other@in.example.com', 'todo'), { ok: false, reason: 'wrong_recipient' });
    assert.deepEqual(parseRecipient('todox+a@in.example.com', 'todo'), { ok: false, reason: 'wrong_recipient' });
    assert.deepEqual(parseRecipient('', 'todo'), { ok: false, reason: 'wrong_recipient' });
  });

  it('honours a configured local part', () => {
    assert.deepEqual(parseRecipient('tasks+home@in.example.com', 'tasks'), { ok: true, list: 'home' });
  });
});

describe('parseAllowlist', () => {
  it('splits on commas, semicolons and whitespace, and lowercases', () => {
    assert.deepEqual([...parseAllowlist(' You@Example.com, second@example.net;third@example.org\nfourth@example.org ')], [
      'you@example.com',
      'second@example.net',
      'third@example.org',
      'fourth@example.org',
    ]);
  });

  it('ignores junk and handles missing values', () => {
    assert.deepEqual([...parseAllowlist('not-an-address, @example.com, ,')], []);
    assert.equal(parseAllowlist(undefined).size, 0);
  });
});

describe('parseSingleMailbox', () => {
  it('parses a mailbox with a display name', () => {
    assert.deepEqual(parseSingleMailbox('"Owner, The" <You@Example.com>'), { address: 'you@example.com', name: 'Owner, The' });
  });

  it('decodes an encoded display name', () => {
    assert.deepEqual(parseSingleMailbox('=?UTF-8?B?RXZhIE5vdsOh?= <you@example.com>'), { address: 'you@example.com', name: 'Eva Nová' });
  });

  it('rejects several mailboxes, as when two From headers are joined', () => {
    assert.equal(parseSingleMailbox('you@example.com, evil@evil.example'), null);
    assert.equal(parseSingleMailbox('Team: you@example.com, b@example.com;'), null);
  });

  it('rejects empty and malformed values', () => {
    assert.equal(parseSingleMailbox(null), null);
    assert.equal(parseSingleMailbox('not an address'), null);
    assert.equal(parseSingleMailbox(`${'a'.repeat(2000)}@example.com`), null);
  });

  it('domainOf', () => {
    assert.equal(domainOf('you@Mail.Example.com'), 'mail.example.com');
    assert.equal(domainOf('nope'), '');
  });
});
