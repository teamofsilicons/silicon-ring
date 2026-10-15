package com.teamofsilicons.ring.call

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.teamofsilicons.ring.ring

/** Decline and hang-up buttons on call notifications. Answering opens the call screen instead. */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val ringid = intent.getStringExtra(EXTRA_RINGID) ?: return
        val calls = context.ring.calls
        when (intent.action) {
            ACTION_DECLINE -> calls.decline(ringid, null)
            ACTION_HANG_UP -> calls.hangUp(ringid)
        }
    }

    companion object {
        const val ACTION_DECLINE = "com.teamofsilicons.ring.action.DECLINE"
        const val ACTION_HANG_UP = "com.teamofsilicons.ring.action.HANG_UP"
        private const val EXTRA_RINGID = "ringid"

        fun intent(context: Context, action: String, ringid: String): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                (action + ringid).hashCode(),
                Intent(context, CallActionReceiver::class.java).setAction(action).putExtra(EXTRA_RINGID, ringid),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
    }
}
