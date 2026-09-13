import { Effect, Layer, Stream } from "effect";
import { decodeWebDavConfigSync, type WebDavConfig } from "./config.ts";
import {
    DeleteFile,
    OperationsLive,
    Exists,
    MoveFile,
    CopyFile,
    CreateDirectory,
    GetDavCompliance,
    GetQuota,
    Stat,
    GetDirectoryContents,
    Search,
    CustomRequest,
    GetFileContents,
    PutFileContents,
    PartialUpdateFileContents,
    CreateReadStream,
    CreateWriteStream,
    Lock,
    Unlock,
    FileLinks,
    type UploadData,
} from "./operations.ts";
import { AuthLive } from "./auth.ts";
import { FetchHttpTransport } from "./transport.ts";

export const ClientLayer = (input: WebDavConfig) => {
    const config = decodeWebDavConfigSync(input);
    return Layer.mergeAll(
        OperationsLive({ config }),
        AuthLive({ config }),
        FetchHttpTransport.layer,
    );
};
export const NodeLayer = ClientLayer;
export const BrowserLayer = ClientLayer;
export const ReactNativeLayer = ClientLayer;

export const makeClient = () =>
    Effect.gen(function* () {
        const deleteFile = yield* DeleteFile;
        const exists = yield* Exists;
        const moveFile = yield* MoveFile;
        const copyFile = yield* CopyFile;
        const createDirectory = yield* CreateDirectory;
        const getDAVCompliance = yield* GetDavCompliance;
        const getQuota = yield* GetQuota;
        const stat = yield* Stat;
        const getDirectoryContents = yield* GetDirectoryContents;
        const search = yield* Search;
        const customRequest = yield* CustomRequest;
        const getFileContents = yield* GetFileContents;
        const putFileContents = yield* PutFileContents;
        const partialUpdateFileContents = yield* PartialUpdateFileContents;
        const createReadStream = yield* CreateReadStream;
        const createWriteStream = yield* CreateWriteStream;
        const lock = yield* Lock;
        const unlock = yield* Unlock;
        const links = yield* FileLinks;
        return {
            deleteFile: deleteFile.execute,
            exists: exists.execute,
            moveFile: moveFile.execute,
            copyFile: copyFile.execute,
            createDirectory: createDirectory.execute,
            getDAVCompliance: getDAVCompliance.execute,
            getQuota: getQuota.execute,
            stat: stat.execute,
            getDirectoryContents: getDirectoryContents.execute,
            search: search.execute,
            customRequest: customRequest.execute,
            getFileContents: getFileContents.execute,
            putFileContents: putFileContents.execute,
            partialUpdateFileContents: partialUpdateFileContents.execute,
            createReadStream: createReadStream.execute,
            createWriteStream: createWriteStream.execute,
            lock: lock.execute,
            unlock: unlock.execute,
            getFileDownloadLink: links.download,
            getFileUploadLink: links.upload,
        };
    });

export const makeLegacyClient = (config: WebDavConfig) =>
    Effect.runPromise(makeClient().pipe(Effect.provide(ClientLayer(config))));

export type Client = Effect.Success<ReturnType<typeof makeClient>>;
export type ClientUploadData = UploadData;
export type ClientStream = Stream.Stream<Uint8Array, unknown>;