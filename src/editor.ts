import {InvalidFilenameError, saveDocument} from './save.js';

export interface Document {
	name: string;
	contents: string;
}

export type SaveResult = {ok: true; path: string} | {ok: false; message: string};

/**
 * Persist the open document. Called from the Cmd+S handler. Invalid names
 * (e.g. all path separators) are reported back instead of throwing, so the
 * save dialog can stay open and show a message.
 */
export function onSave(doc: Document, write: (p: string, d: string) => void): SaveResult {
	try {
		const path = saveDocument({name: doc.name, contents: doc.contents, write});
		return {ok: true, path};
	} catch (error) {
		if (error instanceof InvalidFilenameError) return {ok: false, message: error.message};
		throw error;
	}
}
