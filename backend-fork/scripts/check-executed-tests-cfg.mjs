/**
 * Comment-/literal-aware `#[cfg(test)]` detection for check-executed-tests.
 *
 * Failure class: process.doc-comment-cfg-test-false-dark.
 * A raw `text.includes("#[cfg(test)]")` treats a doc comment that *mentions* the
 * attribute as a live unit-test binary, inventing a false-dark lib entry.
 */

/**
 * Strip line/block comments and string/char/raw-string literals, replacing each
 * with a space. Doc comments (`///`, `//!`, `/**`) are ordinary comments here.
 * String bodies are removed (unlike route-harvest strippers) because an attribute
 * inside a string is never a live cfg gate.
 *
 * @param {string} source
 * @returns {string}
 */
export function stripRustCommentsAndStringLiterals(source) {
  let output = "";
  let index = 0;
  while (index < source.length) {
    const char = source[index];
    const next = source[index + 1];

    if (char === "/" && next === "/") {
      while (index < source.length && source[index] !== "\n") index += 1;
      output += " ";
      continue;
    }
    if (char === "/" && next === "*") {
      let depth = 1;
      index += 2;
      while (index < source.length && depth > 0) {
        if (source[index] === "/" && source[index + 1] === "*") {
          depth += 1;
          index += 2;
        } else if (source[index] === "*" && source[index + 1] === "/") {
          depth -= 1;
          index += 2;
        } else {
          index += 1;
        }
      }
      output += " ";
      continue;
    }
    if (char === "r" && (next === '"' || next === "#")) {
      const raw = source.slice(index).match(/^r(#*)"/);
      if (raw) {
        const terminator = `"${raw[1]}`;
        const end = source.indexOf(terminator, index + raw[0].length);
        index = end === -1 ? source.length : end + terminator.length;
        output += " ";
        continue;
      }
    }
    if (char === "'") {
      const literal = source.slice(index).match(/^'(?:\\.|[^\\'])'/);
      if (literal) {
        index += literal[0].length;
        output += " ";
        continue;
      }
    }
    if (char === '"') {
      index += 1;
      while (index < source.length) {
        const inner = source[index];
        index += 1;
        if (inner === "\\") {
          if (index < source.length) index += 1;
          continue;
        }
        if (inner === '"') break;
      }
      output += " ";
      continue;
    }

    output += char;
    index += 1;
  }
  return output;
}

/**
 * True only when `#[cfg(test)]` appears outside comments and string literals.
 * Deliberately the same literal the gate historically keyed on — does not expand
 * to `#[cfg(all(test, …))]` spellings (those never matched the substring either).
 *
 * @param {string} source
 * @returns {boolean}
 */
export function hasLiveCfgTestAttribute(source) {
  return stripRustCommentsAndStringLiterals(source).includes("#[cfg(test)]");
}

/**
 * Crate `…/src` roots that carry a live `#[cfg(test)]` somewhere under src/.
 *
 * Examined-zero fails closed: an empty src inventory must not read as "no unit
 * tests anywhere" (that would shrink `defined` and clear dark entries).
 *
 * @param {Array<[string, string]>} files repo-relative path + file text pairs
 * @returns {Set<string>}
 */
export function unitTestedCrateSrcRoots(files) {
  const srcFiles = files.filter(([rel]) => rel.includes("/src/"));
  if (srcFiles.length === 0) {
    throw new Error(
      "definedBinaries examined zero backend/**/src/**/*.rs files — refuse empty inventory",
    );
  }
  const crateSrc = (rel) => rel.slice(0, rel.indexOf("/src/") + 4);
  return new Set(
    srcFiles
      .filter(([, text]) => hasLiveCfgTestAttribute(text))
      .map(([rel]) => crateSrc(rel)),
  );
}
