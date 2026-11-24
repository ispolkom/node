package com.yandi.yandi_mobile

import java.nio.ByteBuffer

/** Pure queue framing helpers so bounds and recovery are covered by JVM tests. */
internal object PendingLiveQueue {
    const val MAX_FRAMES = 8
    const val MAX_PAYLOAD = 32 * 1024
    const val FRAME_HEADER = 45
    const val WAKE_TRAILER = 17
    const val MAX_FRAME = FRAME_HEADER + MAX_PAYLOAD + WAKE_TRAILER

    fun decode(bytes: ByteArray): ArrayList<ByteArray> {
        val frames = arrayListOf<ByteArray>()
        var offset = 0
        while (offset + 4 <= bytes.size) {
            val size = ByteBuffer.wrap(bytes, offset, 4).int
            offset += 4
            if (size <= 0 || size > MAX_FRAME || offset + size > bytes.size) break
            frames.add(bytes.copyOfRange(offset, offset + size))
            offset += size
        }
        return frames
    }

    fun append(existing: ByteArray, frame: ByteArray): List<ByteArray> {
        require(frame.isNotEmpty() && frame.size <= MAX_FRAME)
        val frames = decode(existing)
        frames.add(frame.copyOf())
        while (frames.size > MAX_FRAMES) frames.removeAt(0)
        return frames
    }

    fun encode(frames: Collection<ByteArray>): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        for (frame in frames) {
            require(frame.isNotEmpty() && frame.size <= MAX_FRAME)
            out.write(ByteBuffer.allocate(4).putInt(frame.size).array())
            out.write(frame)
        }
        return out.toByteArray()
    }
}

/** Tracks IDs independently; an old hangup can only remove its own wake hint. */
internal class ActiveWakeCalls {
    private val calls = linkedMapOf<String, Boolean>()
    fun invite(id: String, video: Boolean): Boolean {
        val previous = calls[id]
        if (previous != null) {
            if (previous == video) return false
            calls[id] = video
            return true
        }
        if (calls.size == PendingLiveQueue.MAX_FRAMES) calls.remove(calls.keys.first())
        calls[id] = video
        return true
    }
    fun finish(id: String) { calls.remove(id) }
    fun contains(id: String): Boolean = id in calls
    fun isVideo(id: String): Boolean? = calls[id]
}
