import {saveDocument} from './save.js';

export interface Document {
	name: string;
	contents: string;
}

/** Persist the open document. Called from the Cmd+S handler. */
export function onSave(doc: Document, write: (p: string, d: string) => void) {
	return saveDocument({name: doc.name, contents: doc.contents, write});
}
