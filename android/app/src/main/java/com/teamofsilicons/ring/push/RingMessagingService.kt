package com.teamofsilicons.ring.push

import android.os.Handler
import android.os.Looper
import android.util.Log
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import com.teamofsilicons.ring.call.CallNotifications
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.RingJson
import com.teamofsilicons.ring.data.parseInstant
import com.teamofsilicons.ring.ring
import kotlinx.serialization.Serializable

/**
 * FCM high-priority data messages (udd/api.md "Push payloads"). Every top-level value arrives as
 * a string; nested objects are JSON strings.
 */
class RingMessagingService : FirebaseMessagingService() {

    override fun onRegistered(token: String) {
        val graph = ring
        main.post { graph.onPushToken(token) }
    }

    @Deprecated("Superseded by onRegistered in Firebase Messaging 26")
    override fun onNewToken(token: String) {
        onRegistered(token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val data = message.data
        val type = data["type"] ?: return
        val graph = ring
        if (graph.sessions.current == null) return
        // Handle on the main thread but hold this worker until done, so the call service starts
        // inside the window Android grants a high-priority message.
        val done = java.util.concurrent.CountDownLatch(1)
        main.post {
            try {
                handle(type, data)
            } finally {
                done.countDown()
            }
        }
        done.await(5, java.util.concurrent.TimeUnit.SECONDS)
    }

    private fun handle(type: String, data: Map<String, String>) {
        val graph = ring
        when (type) {
            "call.incoming" -> {
                val ringid = data["ringid"] ?: return
                val invitationId = data["invitation_id"] ?: return
                val from = person(data["from"]) ?: return
                val alreadyOn = participants(data["participants"])
                graph.calls.onIncomingPush(ringid, invitationId, from, alreadyOn, parseInstant(data["expires_at"]))
            }
            "call.cancelled" -> {
                val ringid = data["ringid"] ?: return
                graph.calls.onPushCancelled(ringid, data["reason"] ?: "cancelled")
            }
            "call.missed" -> {
                val ringid = data["ringid"] ?: return
                val from = person(data["from"]) ?: return
                CallNotifications.missed(this, ringid, from)
                graph.repo.refreshAll()
            }
            "voicemail.received" -> {
                val id = data["voicemail_id"] ?: return
                val from = person(data["from"]) ?: return
                // FCM carries every value as a string, so a JSON null arrives as the text "null".
                CallNotifications.voicemail(this, id, from, data["transcript"]?.takeUnless { it == "null" })
                graph.repo.refreshAll()
            }
            else -> Log.i(TAG, "Ignoring push type $type")
        }
    }

    @Serializable
    private data class PushPerson(val uuid: String = "", val id: String, val displayName: String? = null, val pfpUrl: String? = null)

    private fun person(json: String?): Account? {
        json ?: return null
        val person = runCatching { RingJson.decodeFromString(PushPerson.serializer(), json) }.getOrNull() ?: return null
        val kind = if (person.id.startsWith("si:")) Account.KIND_SILICON else Account.KIND_CARBON
        return Account(person.uuid.ifEmpty { person.id }, person.id, kind, person.displayName, person.pfpUrl)
    }

    private fun participants(json: String?): List<Account> {
        json ?: return emptyList()
        val list = runCatching {
            RingJson.decodeFromString(kotlinx.serialization.builtins.ListSerializer(PushPerson.serializer()), json)
        }.getOrNull() ?: return emptyList()
        return list.map {
            Account(it.uuid.ifEmpty { it.id }, it.id, if (it.id.startsWith("si:")) Account.KIND_SILICON else Account.KIND_CARBON, it.displayName)
        }
    }

    private companion object {
        const val TAG = "RingPush"
        val main = Handler(Looper.getMainLooper())
    }
}
