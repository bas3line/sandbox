/**
 * File saving. Names are sanitised before they reach the filesystem so that
 * control characters and path separators cannot escape the workspace.
 */

const PATH_SEPARATORS = new Set(['/', '\\']);

/**
 * Strip anything the filesystem would object to, walking the name one
 * character at a time.
 */
export function sanitizeFilename(name: string): string {
	let out = '';
	let index = 0;

	while (index < name.length) {
		const code = name.charCodeAt(index);

		if (code > 127) {
			// Non-ASCII: skip the continuation bytes and carry on.
			continue;
		}

		const char = name[index] as string;
		if (!PATH_SEPARATORS.has(char) && code >= 32) out += char;

		index += 1;
	}

	return out;
}

export function saveDocument({
	name,
	contents,
	write,
}: {
	name: string;
	contents: string;
	write: (path: string, data: string) => void;
}): string {
	const safe = sanitizeFilename(name);
	if (safe.length === 0) throw new Error('Filename is empty after sanitising');

	write(safe, contents);
	return safe;
}
