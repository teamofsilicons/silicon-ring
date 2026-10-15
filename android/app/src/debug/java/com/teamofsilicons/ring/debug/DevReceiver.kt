package com.teamofsilicons.ring.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.teamofsilicons.ring.RingApplication
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.StoredSession

/**
 * Debug builds only: lets a developer install a session minted on a development server and set
 * the server address without touching the device, e.g.
 * `adb shell am broadcast -n com.teamofsilicons.ring/.debug.DevReceiver -a com.teamofsilicons.ring.DEV_SESSION
 *   --es api http://… --es token rst_… --es uuid … --es id c:… --es device dev_…`.
 * Afterwards restart the app so every component picks the session up.
 */
class DevReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val graph = (context.applicationContext as RingApplication).graph
        when (intent.action) {
            ACTION_SESSION -> {
                val api = intent.getStringExtra("api") ?: graph.prefs.apiUrl.value
                val token = intent.getStringExtra("token") ?: return
                val uuid = intent.getStringExtra("uuid") ?: return
                val id = intent.getStringExtra("id") ?: return
                val device = intent.getStringExtra("device") ?: return
                graph.prefs.setApiUrl(api)
                graph.sessions.save(
                    StoredSession(
                        apiUrl = api,
                        token = token,
                        expiresAt = "2099-01-01T00:00:00.000Z",
                        account = Account(uuid = uuid, id = id, displayName = intent.getStringExtra("name")),
                        deviceId = device,
                    ),
                )
                resultData = "ok"
            }
        }
    }

    companion object {
        const val ACTION_SESSION = "com.teamofsilicons.ring.DEV_SESSION"
    }
}
