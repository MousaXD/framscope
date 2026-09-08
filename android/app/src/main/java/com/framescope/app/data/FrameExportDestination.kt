package com.framescope.app.data

import android.content.ContentResolver
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import java.io.Closeable

interface FrameExportDestination : Closeable {
    val uri: String
    val fd: Int
    fun commit()
}

interface FrameExportDestinationFactory {
    fun create(
        treeUri: String,
        displayName: String,
        mimeType: String,
    ): FrameExportDestination
}

class AndroidFrameExportDestinationFactory(
    private val contentResolver: ContentResolver,
) : FrameExportDestinationFactory {
    override fun create(
        treeUri: String,
        displayName: String,
        mimeType: String,
    ): FrameExportDestination {
        require(displayName.isNotBlank()) { "Export display name must not be blank." }
        require(mimeType.startsWith("image/")) { "Export MIME type must be an image type." }
        val tree = Uri.parse(treeUri)
        require(tree.scheme == ContentResolver.SCHEME_CONTENT) {
            "Export destination must be a content:// document tree."
        }
        val treeDocumentId = DocumentsContract.getTreeDocumentId(tree)
        val parent = DocumentsContract.buildDocumentUriUsingTree(tree, treeDocumentId)
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
        return AndroidFrameExportDestination(contentResolver, child, descriptor)
    }
}

private class AndroidFrameExportDestination(
    private val contentResolver: ContentResolver,
    private val documentUri: Uri,
    private val descriptor: ParcelFileDescriptor,
) : FrameExportDestination {
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
