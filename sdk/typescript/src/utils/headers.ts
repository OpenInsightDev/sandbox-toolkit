import type { Headers } from "#/domain.ts";

const lowerCaseHeaders = (headers: Headers): Map<string, string> =>
    new Map(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), value]));

export const getHeader = (headers: Headers, name: string): string | undefined =>
    lowerCaseHeaders(headers).get(name.toLowerCase());