import assert from 'node:assert/strict';
import {test} from 'node:test';

import {onSave} from './editor.js';

test('onSave returns ok with the saved path for a valid name', () => {
	let written: [string, string] | undefined;
	const result = onSave({name: 'notes.txt', contents: 'hi'}, (path, data) => {
		written = [path, data];
	});

	assert.deepEqual(result, {ok: true, path: 'notes.txt'});
	assert.deepEqual(written, ['notes.txt', 'hi']);
});

test('onSave returns a validation failure instead of throwing for slash-only names', () => {
	let wrote = false;
	const result = onSave({name: '///', contents: 'hi'}, () => {
		wrote = true;
	});

	assert.equal(result.ok, false);
	assert.equal(wrote, false);
});
