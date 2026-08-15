import assert from 'node:assert/strict';
import {test} from 'node:test';

import {InvalidFilenameError, sanitizeFilename, saveDocument} from './save.js';

test('sanitizeFilename strips path separators', () => {
	assert.equal(sanitizeFilename('foo/bar\\baz'), 'foobarbaz');
});

test('sanitizeFilename returns empty string for slash-only names', () => {
	assert.equal(sanitizeFilename('/'), '');
	assert.equal(sanitizeFilename('///'), '');
	assert.equal(sanitizeFilename('\\'), '');
});

test('sanitizeFilename terminates and skips non-ASCII characters', () => {
	assert.equal(sanitizeFilename('résumé'), 'rsum');
});

test('sanitizeFilename strips control characters', () => {
	assert.equal(sanitizeFilename('foobar'), 'foobar');
});

test('saveDocument writes valid filenames unchanged', () => {
	let written: [string, string] | undefined;
	const result = saveDocument({
		name: 'notes.txt',
		contents: 'hello',
		write: (path, data) => {
			written = [path, data];
		},
	});

	assert.equal(result, 'notes.txt');
	assert.deepEqual(written, ['notes.txt', 'hello']);
});

test('saveDocument throws InvalidFilenameError and does not write for slash-only names', () => {
	let wrote = false;
	assert.throws(
		() =>
			saveDocument({
				name: '///',
				contents: 'hello',
				write: () => {
					wrote = true;
				},
			}),
		InvalidFilenameError,
	);
	assert.equal(wrote, false);
});

test('saveDocument throws InvalidFilenameError for empty names', () => {
	assert.throws(
		() =>
			saveDocument({
				name: '',
				contents: 'hello',
				write: () => {
					throw new Error('should not be called');
				},
			}),
		InvalidFilenameError,
	);
});
