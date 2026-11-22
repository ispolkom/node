package com.yandi.yandi_mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PendingLiveQueueTest {
    @Test fun `queue retains newest eight frames and restores them in order`() {
        val frames = (1..9).map { byteArrayOf(it.toByte()) }
        val queue = frames.fold(ByteArray(0)) { stored, frame ->
            PendingLiveQueue.encode(PendingLiveQueue.append(stored, frame))
        }
        assertEquals((2..9).map { it.toByte() }.toList(),
            PendingLiveQueue.decode(queue).map { it.single() })
    }

    @Test fun `corrupt tail is discarded and new frame is re-encoded`() {
        val valid = PendingLiveQueue.encode(listOf(byteArrayOf(1), byteArrayOf(2)))
        val corrupt = valid + byteArrayOf(0, 0, 0, 5, 9)
        val recovered = PendingLiveQueue.append(corrupt, byteArrayOf(3))
        assertEquals(listOf(1, 2, 3), recovered.map { it.single().toInt() })
        assertEquals(listOf(1, 2, 3),
            PendingLiveQueue.decode(PendingLiveQueue.encode(recovered)).map { it.single().toInt() })
    }

    @Test fun `one call hangup does not clear another and repeated invites are idempotent`() {
        val wakes = ActiveWakeCalls()
        assertTrue(wakes.invite("call-a", video = false))
        assertFalse(wakes.invite("call-a", video = false))
        assertTrue(wakes.invite("call-b", video = true))
        assertTrue(wakes.invite("call-b", video = false))
        assertEquals(false, wakes.isVideo("call-b"))
        wakes.finish("call-a")
        assertFalse(wakes.contains("call-a"))
        assertTrue(wakes.contains("call-b"))
        wakes.finish("stale-call")
        assertTrue(wakes.contains("call-b"))
        assertTrue(wakes.invite("call-a", video = true))
    }

    @Test fun `wake id tracking stays bounded during an invite flood`() {
        val wakes = ActiveWakeCalls()
        repeat(PendingLiveQueue.MAX_FRAMES + 3) { wakes.invite("call-$it", video = it % 2 == 0) }
        assertFalse(wakes.contains("call-0"))
        assertTrue(wakes.contains("call-10"))
    }
}
