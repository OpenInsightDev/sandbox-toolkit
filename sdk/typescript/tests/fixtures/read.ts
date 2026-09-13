import { readFileSync } from "node:fs";

/** Read one of the golden WebDAV response fixtures migrated from the reference suite. */
export const readResponse = (name: string): string =>
    readFileSync(new URL(`./responses/${name}.xml`, import.meta.url), "utf8");