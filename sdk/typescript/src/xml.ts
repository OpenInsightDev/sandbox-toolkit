import { Effect, Schema } from "effect";
import XMLBuilder from "fast-xml-builder";
import { XMLParser, type X2jOptions } from "fast-xml-parser";
import { SyntaxValidator } from "fast-xml-validator";
import { DAVResult, type DAVResult as DavMultiStatus } from "#/domain.ts";
import { XmlEncodeError, XmlParseError, ResponseDecodeError } from "#/error.ts";

/** The intentionally permissive boundary shape returned by fast-xml-parser. */
export const RawDavXml = Schema.Unknown;
export type RawDavXml = Schema.Schema.Type<typeof RawDavXml>;

export const XmlParseOptions = Schema.Struct({
    clarkNotation: Schema.optional(Schema.Boolean),
    maxXmlLength: Schema.optional(Schema.Int),
    maxDepth: Schema.optional(Schema.Int),
    entityDecoder: Schema.optional(
        Schema.Struct({
            limit: Schema.optional(
                Schema.Struct({
                    maxTotalExpansions: Schema.optional(Schema.Int),
                    maxExpandedLength: Schema.optional(Schema.Int),
                }),
            ),
        }),
    ),
});
export type XmlParseOptions = Schema.Schema.Type<typeof XmlParseOptions>;

export const LockInfo = Schema.Struct({ ownerHref: Schema.String });
export type LockInfo = Schema.Schema.Type<typeof LockInfo>;

const DAV_NS = "DAV:";
const errorMessage = (operation: string, message: string, cause?: unknown) => ({
    operation,
    message,
    ...(cause instanceof Error ? { cause: cause.message } : {}),
});

const ensureArray = <A>(value: A | ReadonlyArray<A> | null | undefined): ReadonlyArray<A> => {
    if (value == null) return [];
    if (Array.isArray(value)) return value as ReadonlyArray<A>;
    return [value as A];
};

const text = (value: unknown): string | undefined => {
    if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
        return String(value);
    }
    if (value && typeof value === "object" && "#text" in value) return text(value["#text"]);
    return undefined;
};

const defaultDavPrefixes: ReadonlySet<string> = new Set(["D", "DAV"]);

/**
 * Collect every prefix (including the empty default namespace) that the document
 * binds to the DAV: namespace. Servers serialise the namespace with arbitrary
 * prefixes (`D:`, `d:`, `ns0:`), so key normalization must resolve them instead
 * of hard-coding a fixed prefix list.
 */
export const collectDavPrefixes = (xml: string): ReadonlySet<string> => {
    const prefixes = new Set<string>(defaultDavPrefixes);
    const declaration = /xmlns(?::([A-Za-z_][\w.-]*))?\s*=\s*["']\s*DAV:\s*["']/g;
    let match = declaration.exec(xml);
    while (match !== null) {
        prefixes.add(match[1] ?? "");
        match = declaration.exec(xml);
    }
    return prefixes;
};

const isAttributeKey = (key: string): boolean => key.startsWith("@_");
const isXmlnsKey = (key: string): boolean => key === "@_xmlns" || key.startsWith("@_xmlns:");

const isDavKey = (key: string, prefixes: ReadonlySet<string>): boolean => {
    if (key.startsWith("{DAV:}")) return true;
    if (key.startsWith("{")) return false;
    const colon = key.indexOf(":");
    return prefixes.has(colon === -1 ? "" : key.slice(0, colon));
};

const localName = (key: string, prefixes: ReadonlySet<string>): string => {
    if (key.startsWith("{DAV:}")) return key.slice("{DAV:}".length);
    if (key.startsWith("{")) return key;
    return isDavKey(key, prefixes) ? key.slice(key.indexOf(":") + 1) : key;
};

/** Recursively normalize a property value, stripping DAV prefixes but keeping extension namespaces. */
const normalizePropValue = (raw: unknown, prefixes: ReadonlySet<string>): unknown => {
    if (raw === null || raw === undefined) return raw;
    if (Array.isArray(raw)) return raw.map((item) => normalizePropValue(item, prefixes));
    if (typeof raw !== "object") return text(raw);
    const record = raw as Record<string, unknown>;
    const textValue = record["#text"];
    const entries = Object.entries(record).filter(([key]) => key !== "#text" && !isXmlnsKey(key));
    if (entries.length === 0) {
        return textValue === undefined ? "" : text(textValue);
    }
    const result: Record<string, unknown> = {};
    for (const [key, value] of entries) {
        const name = isAttributeKey(key) ? key : localName(key, prefixes);
        result[name] = normalizePropValue(value, prefixes);
    }
    if (typeof textValue === "string" && textValue.trim() !== "") {
        result.text = textValue;
    } else if (typeof textValue === "number" || typeof textValue === "boolean") {
        result.text = text(textValue);
    }
    return result;
};

const normalizeResourceType = (
    raw: unknown,
    prefixes: ReadonlySet<string>,
): Record<string, unknown> => {
    if (!raw || typeof raw !== "object") return {};
    const result: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
        if (isAttributeKey(key) || key === "#text") continue;
        result[localName(key, prefixes)] = normalizePropValue(value, prefixes);
    }
    return result;
};

const normalizeProperties = (
    value: unknown,
    prefixes: ReadonlySet<string>,
): Record<string, unknown> => {
    if (!value || typeof value !== "object") return {};
    const result: Record<string, unknown> = {};
    for (const [key, raw] of Object.entries(value)) {
        if (isAttributeKey(key) || key === "#text") continue;
        const name = localName(key, prefixes);
        result[name] =
            name === "resourcetype"
                ? normalizeResourceType(raw, prefixes)
                : normalizePropValue(raw, prefixes);
    }
    return result;
};

const normalizeRawDavResult = (
    raw: RawDavXml,
    prefixes: ReadonlySet<string> = defaultDavPrefixes,
): unknown => {
    if (!raw || typeof raw !== "object") return raw;
    const root = raw as Record<string, unknown>;
    const rootKey = Object.keys(root).find((key) => localName(key, prefixes) === "multistatus");
    const multistatus = rootKey ? root[rootKey] : root;
    const container =
        multistatus && typeof multistatus === "object"
            ? (multistatus as Record<string, unknown>)
            : {};
    const responses = ensureArray(
        container[
            Object.keys(container).find((key) => localName(key, prefixes) === "response") ??
                "response"
        ],
    );
    return {
        multistatus: {
            response: responses.map((item) => {
                const response =
                    item && typeof item === "object" ? (item as Record<string, unknown>) : {};
                const get = (name: string) =>
                    response[
                        Object.keys(response).find((key) => localName(key, prefixes) === name) ??
                            name
                    ];
                const propstats = ensureArray(get("propstat"));
                const propstat = propstats[0] as Record<string, unknown> | undefined;
                const props =
                    propstat?.[
                        Object.keys(propstat).find((key) => localName(key, prefixes) === "prop") ??
                            "prop"
                    ];
                return {
                    href: text(get("href")) ?? "",
                    ...(text(get("status")) === undefined ? {} : { status: text(get("status")) }),
                    ...(text(get("responsedescription")) === undefined
                        ? {}
                        : { responsedescription: text(get("responsedescription")) }),
                    ...(propstat === undefined
                        ? {}
                        : {
                              propstat: {
                                  prop: normalizeProperties(props, prefixes),
                                  status:
                                      text(
                                          propstat[
                                              Object.keys(propstat).find(
                                                  (key) => localName(key, prefixes) === "status",
                                              ) ?? "status"
                                          ],
                                      ) ?? "",
                                  ...(text(
                                      propstat[
                                          Object.keys(propstat).find(
                                              (key) =>
                                                  localName(key, prefixes) ===
                                                  "responsedescription",
                                          ) ?? "responsedescription"
                                      ],
                                  ) === undefined
                                      ? {}
                                      : {
                                            responsedescription: text(
                                                propstat[
                                                    Object.keys(propstat).find(
                                                        (key) =>
                                                            localName(key, prefixes) ===
                                                            "responsedescription",
                                                    ) ?? "responsedescription"
                                                ],
                                            ),
                                        }),
                              },
                          }),
                };
            }),
        },
    };
};

const parserOptions = (
    options: XmlParseOptions,
    davPrefixes: ReadonlySet<string> = defaultDavPrefixes,
): X2jOptions => ({
    ignoreAttributes: false,
    attributeNamePrefix: "@_",
    textNodeName: "#text",
    trimValues: false,
    parseTagValue: false,
    parseAttributeValue: false,
    allowBooleanAttributes: true,
    processEntities: true,
    stopNodes: [],
    ...(options.clarkNotation
        ? {
              transformTagName: (name: string) => {
                  if (name.startsWith("{")) return name;
                  const colon = name.indexOf(":");
                  const prefix = colon === -1 ? "" : name.slice(0, colon);
                  if (!davPrefixes.has(prefix)) return name;
                  return `{${DAV_NS}}${colon === -1 ? name : name.slice(colon + 1)}`;
              },
          }
        : {}),
});

export interface DavXmlCodec {
    readonly parseMultiStatus: (
        xml: string,
        options?: XmlParseOptions,
    ) => Effect.Effect<DavMultiStatus, XmlParseError | ResponseDecodeError>;
    readonly buildLockInfo: (ownerHref: string) => Effect.Effect<string, XmlEncodeError>;
    readonly parseGeneric: (
        xml: string,
        options?: XmlParseOptions,
    ) => Effect.Effect<unknown, XmlParseError>;
}

export const ensureDavArray = ensureArray;
export const normalizeDavXml = normalizeRawDavResult;

export const DavXmlCodecLive: DavXmlCodec = {
    parseGeneric: (xml, options = {}) =>
        Effect.try({
            try: () => {
                const decoded = Schema.decodeUnknownSync(XmlParseOptions)(options);
                if (decoded.maxXmlLength !== undefined && xml.length > decoded.maxXmlLength)
                    throw new Error("XML body exceeds configured limit");
                if (/<!DOCTYPE|<!ENTITY/i.test(xml))
                    throw new Error("XML entities are not allowed");
                if (decoded.entityDecoder?.limit?.maxTotalExpansions !== undefined) {
                    const references = (xml.match(/&(?:#\d+|#x[\da-f]+|[a-z][\w.-]*);/gi) ?? [])
                        .length;
                    if (references > decoded.entityDecoder.limit.maxTotalExpansions)
                        throw new Error("XML entity expansion limit exceeded");
                }
                const validation = SyntaxValidator.validate(xml, { allowBooleanAttributes: true });
                if (validation !== true) throw new Error("XML is not well formed");
                return new XMLParser(parserOptions(decoded, collectDavPrefixes(xml))).parse(
                    xml,
                ) as unknown;
            },
            catch: (cause) =>
                new XmlParseError(errorMessage("xml.parse", "Unable to parse XML", cause)),
        }),
    parseMultiStatus: (xml, options = {}) =>
        DavXmlCodecLive.parseGeneric(xml, options).pipe(
            Effect.map((value) => normalizeRawDavResult(value, collectDavPrefixes(xml))),
            Effect.flatMap((value) =>
                Schema.decodeUnknownEffect(DAVResult)(value).pipe(
                    Effect.mapError(
                        (cause) =>
                            new ResponseDecodeError(
                                errorMessage(
                                    "xml.decode",
                                    "Invalid DAV multistatus response",
                                    cause,
                                ),
                            ),
                    ),
                ),
            ),
        ),
    buildLockInfo: (ownerHref) =>
        Effect.try({
            try: () => {
                Schema.decodeUnknownSync(Schema.String)(ownerHref);
                return new XMLBuilder({ format: false, ignoreAttributes: false }).build({
                    "D:lockinfo": {
                        "@_xmlns:D": DAV_NS,
                        "D:lockscope": { "D:exclusive": "" },
                        "D:locktype": { "D:write": "" },
                        "D:owner": { "D:href": ownerHref },
                    },
                });
            },
            catch: (cause) =>
                new XmlEncodeError(errorMessage("xml.encode", "Unable to build lock XML", cause)),
        }),
};

export const DavXmlCodecTest = (): DavXmlCodec => DavXmlCodecLive;