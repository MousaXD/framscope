package com.framescope.app.data

import android.content.ContentResolver
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import java.io.Closeable

interface ExportDocument : Closeable {
    val uri: String
    val fd: Int
    fun commit()
}

interface ExportDocumentFactory {
    fun create(
        treeUri: String,
        displayName: String,
        mimeType: String,
    ): ExportDocument
}

interface ExportWorkspace : Closeable {
    val treeUri: String
    fun commit()
}

interface ExportWorkspaceFactory {
    fun create(
        parentTreeUri: String,
        displayName: String,
    ): ExportWorkspace
}

/**
 * Batch-aware SAF document factory.
 *
 * The manifest is always created first by the repository. That call opens one fresh workspace
 * directory and keeps it active until the manifest is closed. Frame documents created while that
 * manifest is active are transparently routed into the same workspace. Committing the manifest
 * commits the workspace; closing an uncommitted manifest deletes the whole workspace.
 *
 * This prevents Android document providers from silently renaming colliding frame files in the
 * caller's long-lived parent tree while preserving the native manifest's stable relative names.
 */
class AndroidExportDocumentFactory(
    private val contentResolver: ContentResolver,
) : ExportDocumentFactory {
    private data class ActiveBatch(
        val parentTreeUri: String,
        val workspace: ExportWorkspace,
    )

    private val stateLock = Any()
    private val workspaceFactory: ExportWorkspaceFactory = AndroidExportWorkspaceFactory(contentResolver)
    private var activeBatch: ActiveBatch? = null

    override fun create(
        treeUri: String,
        displayName: String,
        mimeType: String,
    ): ExportDocument {
        validateDisplayName(displayName)
        validateMimeType(mimeType)
        return synchronized(stateLock) {
            if (isBatchManifest(displayName, mimeType)) {
                createBatchManifest(treeUri, displayName, mimeType)
            } else {
                val batch = activeBatch
                if (batch != null && batch.parentTreeUri != treeUri) {
                    throw IllegalStateException("A batch export workspace is already active for another tree.")
                }
                createPlainDocument(
                    treeUri = batch?.workspace?.treeUri ?: treeUri,
                    displayName = displayName,
                    mimeType = mimeType,
                )
            }
        }
    }

    private fun createBatchManifest(
        parentTreeUri: String,
        displayName: String,
        mimeType: String,
    ): ExportDocument {
        check(activeBatch == null) { "A batch export workspace is already active." }
        val workspace = workspaceFactory.create(
            parentTreeUri = parentTreeUri,
            displayName = workspaceDisplayName(displayName),
        )
        val batch = ActiveBatch(parentTreeUri = parentTreeUri, workspace = workspace)
        activeBatch = batch
        val document = try {
            createPlainDocument(
                treeUri = workspace.treeUri,
                displayName = displayName,
                mimeType = mimeType,
            )
        } catch (error: Throwable) {
            activeBatch = null
            runCatching { workspace.close() }
            throw error
        }
        return BatchManifestDocument(
            document = document,
            workspace = workspace,
            onClosed = { clearBatch(batch) },
        )
    }

    private fun createPlainDocument(
        treeUri: String,
        displayName: String,
        mimeType: String,
    ): ExportDocument {
        val tree = requireContentUri(treeUri)
        val parent = parentDocumentUri(tree)
        val child = DocumentsContract.createDocument(
            contentResolver,
            parent,
            mimeType,
            displayName,
        ) ?: throw IllegalStateException("Android document provider could not create the export file.")

        val descriptor = try {
            contentResolver.openFileDescriptor(child, "wt")
                ?: throw IllegalStateException("Android document provider returned no writable descriptor.")
        } catch (error: Throwable) {
            runCatching { DocumentsContract.deleteDocument(contentResolver, child) }
            throw error
        }
        return AndroidExportDocument(contentResolver, child, descriptor)
    }

    private fun clearBatch(batch: ActiveBatch) {
        synchronized(stateLock) {
            if (activeBatch === batch) {
                activeBatch = null
            }
        }
    }

    private fun isBatchManifest(displayName: String, mimeType: String): Boolean =
        mimeType == BATCH_MANIFEST_MIME_TYPE &&
            displayName.startsWith(BATCH_MANIFEST_PREFIX) &&
            displayName.endsWith(BATCH_MANIFEST_SUFFIX)

    private fun workspaceDisplayName(manifestDisplayName: String): String {
        val identity = manifestDisplayName
            .removePrefix(BATCH_MANIFEST_PREFIX)
            .removeSuffix(BATCH_MANIFEST_SUFFIX)
        return "$BATCH_WORKSPACE_PREFIX$identity"
    }

    private companion object {
        const val BATCH_MANIFEST_MIME_TYPE = "application/json"
        const val BATCH_MANIFEST_PREFIX = "framescope_manifest_"
        const val BATCH_MANIFEST_SUFFIX = ".jsonl"
        const val BATCH_WORKSPACE_PREFIX = "framescope_export_"
    }
}

class AndroidExportWorkspaceFactory(
    private val contentResolver: ContentResolver,
) : ExportWorkspaceFactory {
    override fun create(
        parentTreeUri: String,
        displayName: String,
    ): ExportWorkspace {
        validateDisplayName(displayName)
        val tree = requireContentUri(parentTreeUri)
        val parent = parentDocumentUri(tree)
        val directory = DocumentsContract.createDocument(
            contentResolver,
            parent,
            DocumentsContract.Document.MIME_TYPE_DIR,
            displayName,
        ) ?: throw IllegalStateException("Android document provider could not create the batch export folder.")
        return AndroidExportWorkspace(contentResolver, directory)
    }
}

private class AndroidExportDocument(
    private val contentResolver: ContentResolver,
    private val documentUri: Uri,
    private val descriptor: ParcelFileDescriptor,
) : ExportDocument {
    private var committed = false
    private var closed = false

    override val uri: String = documentUri.toString()
    override val fd: Int
        get() {
            check(!closed) { "Export destination is already closed." }
            return descriptor.fd
        }

    override fun commit() {
        check(!closed) { "Cannot commit a closed export destination." }
        committed = true
    }

    override fun close() {
        if (closed) return
        closed = true
        try {
            descriptor.close()
        } finally {
            if (!committed) {
                runCatching { DocumentsContract.deleteDocument(contentResolver, documentUri) }
            }
        }
    }
}

private class AndroidExportWorkspace(
    private val contentResolver: ContentResolver,
    private val directoryUri: Uri,
) : ExportWorkspace {
    private var committed = false
    private var closed = false

    override val treeUri: String = directoryUri.toString()

    override fun commit() {
        check(!closed) { "Cannot commit a closed export workspace." }
        committed = true
    }

    override fun close() {
        if (closed) return
        closed = true
        if (!committed) {
            runCatching { DocumentsContract.deleteDocument(contentResolver, directoryUri) }
        }
    }
}

private class BatchManifestDocument(
    private val document: ExportDocument,
    private val workspace: ExportWorkspace,
    private val onClosed: () -> Unit,
) : ExportDocument {
    private var committed = false
    private var closed = false

    override val uri: String
        get() = document.uri

    override val fd: Int
        get() = document.fd

    override fun commit() {
        check(!closed) { "Cannot commit a closed batch manifest." }
        document.commit()
        workspace.commit()
        committed = true
    }

    override fun close() {
        if (closed) return
        closed = true
        try {
            document.close()
        } finally {
            try {
                workspace.close()
            } finally {
                onClosed()
            }
        }
    }
}

/**
 * JNI-visible SAF sink for a streaming batch export.
 *
 * Rust calls these methods synchronously. The sink therefore retains at most one caller-owned
 * document descriptor and never accumulates frame URIs or descriptors for the whole batch.
 */
class BatchSafFrameSink(
    private val treeUri: String,
    private val documentFactory: ExportDocumentFactory,
    private val onProgress: (BatchExportProgress) -> Unit = {},
) : NativeBatchFrameSink, Closeable {
    private data class Pending(
        val fileName: String,
        val frameId: Long,
        val ordinal: Long,
        val total: Long,
        val document: ExportDocument,
    )

    private var pending: Pending? = null
    private var closed = false

    override fun openFrame(
        fileName: String,
        mimeType: String,
        frameId: Long,
        ordinal: Long,
        total: Long,
    ): Int {
        if (closed || pending != null) return INVALID_FD
        val progress = BatchExportProgress(frameId = frameId, ordinal = ordinal, total = total)
        if (!progress.isSane() || !validFrameName(fileName) || !mimeType.startsWith("image/")) {
            return INVALID_FD
        }

        val document = try {
            documentFactory.create(
                treeUri = treeUri,
                displayName = fileName,
                mimeType = mimeType,
            )
        } catch (_: Throwable) {
            return INVALID_FD
        }
        val value = Pending(
            fileName = fileName,
            frameId = frameId,
            ordinal = ordinal,
            total = total,
            document = document,
        )
        pending = value
        return try {
            onProgress(progress)
            document.fd.takeIf { it >= 0 } ?: run {
                abortPending(value)
                INVALID_FD
            }
        } catch (_: Throwable) {
            abortPending(value)
            INVALID_FD
        }
    }

    override fun commitFrame(
        fileName: String,
        frameId: Long,
        ordinal: Long,
        total: Long,
        byteLength: Long,
    ): Boolean {
        if (closed || byteLength <= 0L) return false
        val value = pending ?: return false
        if (
            value.fileName != fileName ||
            value.frameId != frameId ||
            value.ordinal != ordinal ||
            value.total != total
        ) {
            return false
        }

        pending = null
        return try {
            value.document.commit()
            value.document.close()
            true
        } catch (_: Throwable) {
            runCatching { value.document.close() }
            false
        }
    }

    override fun abortFrame(fileName: String) {
        val value = pending ?: return
        if (value.fileName != fileName) return
        abortPending(value)
    }

    override fun close() {
        if (closed) return
        closed = true
        pending?.let(::abortPending)
    }

    private fun abortPending(value: Pending) {
        if (pending === value) pending = null
        runCatching { value.document.close() }
    }

    private fun validFrameName(fileName: String): Boolean =
        fileName.isNotBlank() &&
            fileName.length <= MAX_FRAME_FILE_NAME_LENGTH &&
            '/' !in fileName &&
            '\\' !in fileName

    private companion object {
        const val INVALID_FD = -1
        const val MAX_FRAME_FILE_NAME_LENGTH = 240
    }
}

private fun requireContentUri(value: String): Uri {
    val uri = Uri.parse(value)
    require(uri.scheme == ContentResolver.SCHEME_CONTENT) {
        "Export destination must be a content:// document tree or document."
    }
    return uri
}

private fun parentDocumentUri(uri: Uri): Uri {
    val documentId = runCatching { DocumentsContract.getDocumentId(uri) }
        .getOrElse { DocumentsContract.getTreeDocumentId(uri) }
    return DocumentsContract.buildDocumentUriUsingTree(uri, documentId)
}

private fun validateDisplayName(displayName: String) {
    require(displayName.isNotBlank() && displayName.length <= MAX_DISPLAY_NAME_LENGTH) {
        "Export display name is empty or exceeds the safety limit."
    }
    require('/' !in displayName && '\\' !in displayName) {
        "Export display name must not contain path separators."
    }
}

private fun validateMimeType(mimeType: String) {
    require(mimeType.isNotBlank() && mimeType.length <= MAX_MIME_TYPE_LENGTH) {
        "Export MIME type is empty or exceeds the safety limit."
    }
}

private const val MAX_DISPLAY_NAME_LENGTH = 240
private const val MAX_MIME_TYPE_LENGTH = 128
