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
import kotlinx.coroutines.launch
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
                val ringid = data.value("ringid") ?: return
                val invitationId = data.value("invitation_id") ?: return
                // Ring even if the caller is unreadable: the controller loads the call right away
                // and puts the real name in place.
                val from = person(data.value("caller") ?: data.value("from"))
                    ?: Account(uuid = "", id = "", displayName = "Someone")
                val alreadyOn = participants(data.value("participants"))
                graph.calls.onIncomingPush(ringid, invitationId, from, alreadyOn, parseInstant(data.value("expires_at")))
            }
            "call.cancelled" -> {
                val ringid = data.value("ringid") ?: return
                graph.calls.onPushCancelled(ringid, data.value("invitation_id"), data.value("reason") ?: "cancelled")
            }
            "call.missed" -> {
                val ringid = data.value("ringid") ?: return
                val from = person(data.value("caller") ?: data.value("from"))
                if (from != null) {
                    graph.calls.notifyMissed(ringid, from)
                } else {
                    graph.scope.launch {
                        // The caller is whoever invited this account (not always who started the call).
                        val me = graph.repo.myUuid.orEmpty()
                        runCatching { graph.api.call(ringid) }
                            .onSuccess { graph.calls.notifyMissed(ringid, it.latestInvitationFor(me)?.from ?: it.createdBy) }
                    }
                }
                graph.repo.refreshAll()
            }
            "voicemail.received" -> {
                val id = data.value("voicemail_id") ?: return
                val from = person(data.value("caller") ?: data.value("from"))
                if (from != null) {
                    CallNotifications.voicemail(this, id, from, data.value("transcript"))
                } else {
                    val context = applicationContext
                    graph.scope.launch {
                        runCatching { graph.api.voicemail(id) }
                            .onSuccess { CallNotifications.voicemail(context, id, it.from, it.transcript) }
                    }
                }
                graph.repo.refreshAll()
            }
            else -> Log.i(TAG, "Ignoring push type $type")
        }
    }

    /** A data value; FCM carries every value as a string, so a JSON null arrives as "null". */
    private fun Map<String, String>.value(key: String): String? = get(key)?.takeUnless { it.isBlank() || it == "null" }

    @Serializable
    private data class PushPerson(
        val uuid: String = "",
        val id: String = "",
        val kind: String? = null,
        val displayName: String? = null,
        val pfpUrl: String? = null,
    ) {
        fun toAccount(): Account? {
            if (uuid.isEmpty() && id.isEmpty()) return null
            val kind = when (kind) {
                Account.KIND_SILICON, Account.KIND_CARBON -> kind
                else -> if (id.startsWith("si:")) Account.KIND_SILICON else Account.KIND_CARBON
            }
            return Account(uuid.ifEmpty { id }, id.ifEmpty { uuid }, kind, displayName, pfpUrl)
        }
    }

    private fun person(json: String?): Account? {
        json ?: return null
        return runCatching { RingJson.decodeFromString(PushPerson.serializer(), json) }.getOrNull()?.toAccount()
    }

    private fun participants(json: String?): List<Account> {
        json ?: return emptyList()
        val list = runCatching {
            RingJson.decodeFromString(kotlinx.serialization.builtins.ListSerializer(PushPerson.serializer()), json)
        }.getOrNull() ?: return emptyList()
        return list.mapNotNull { it.toAccount() }
    }

    private companion object {
        const val TAG = "RingPush"
        val main = Handler(Looper.getMainLooper())
    }
}
