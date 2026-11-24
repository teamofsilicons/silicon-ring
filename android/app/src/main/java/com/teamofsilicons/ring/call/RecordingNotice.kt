package com.teamofsilicons.ring.call

import com.teamofsilicons.ring.data.AppPrefs
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The notice that Ring records and transcribes calls, and which providers process the audio. Each
 * device shows it once right after sign-in, and again before a call until it has agreed, and
 * remembers the answer ([AppPrefs], which is never backed up, so it stays with this device).
 *
 * A call that goes on here before this device agreed (answered from Telecom or from the
 * notification) keeps its microphone held ([CallController], [CallMedia]) and shows the notice on
 * its call screen; agreeing releases the microphone.
 */
class RecordingNotice(private val prefs: AppPrefs) {

    /** The call action waiting for the notice. */
    sealed interface Action {
        /** Right after sign-in, before any call: nothing waits. */
        data object Welcome : Action
        /** Calling someone. */
        data object Place : Action
        /** Answering this ring in the app. */
        data class Answer(val ringid: String) : Action
        /** Moving this call here from another device. */
        data class SwitchHere(val ringid: String) : Action
        /**
         * A call this device carried before it agreed. While it is live the notice shows on its
         * call screen ([CallUi.waitsForNotice]); if it ends first, the notice shows in the app.
         */
        data object Answered : Action
    }

    /** The notice on screen, the action waiting for it, and what runs on "Agree and continue". */
    class Request internal constructor(val action: Action, internal val owner: Any?, internal val then: () -> Unit)

    private val _accepted = MutableStateFlow(prefs.recordingNoticeAccepted >= VERSION)
    /** This device agreed to the current notice. */
    val accepted: StateFlow<Boolean> = _accepted.asStateFlow()

    private val _request = MutableStateFlow<Request?>(null)
    val request: StateFlow<Request?> = _request.asStateFlow()

    /** This device has not agreed to the current notice yet. */
    val needed: Boolean get() = !_accepted.value

    /**
     * After sign-in, and on a launch signed in after the notice changed: the notice once, before
     * the home screen can be used, so that this device has normally agreed before a call can be
     * answered from Telecom or the notification.
     */
    fun offer() {
        if (!needed || prefs.recordingNoticeOffered >= VERSION || _request.value != null) return
        _request.value = Request(Action.Welcome, null) {}
    }

    /**
     * Runs [then] now when this device has agreed; otherwise shows the notice for [action] and runs
     * it on "Agree and continue". [owner] withdraws the request when it goes away ([withdraw]).
     */
    fun before(action: Action, owner: Any?, then: () -> Unit) {
        if (!needed) return then()
        _request.value = Request(action, owner, then)
    }

    /**
     * A call goes on here with its microphone held: its notice takes the place of whatever call
     * action was waiting, since that call now comes first.
     */
    fun showOverCall() {
        if (needed) _request.value = Request(Action.Answered, null) {}
    }

    /** "Agree and continue": remembered on this device; the waiting action, if any, carries on. */
    fun agree() {
        prefs.recordingNoticeAccepted = VERSION
        _accepted.value = true
        val request = _request.value
        _request.value = null
        request?.then?.invoke()
    }

    /**
     * "Not now": drops the waiting action. The notice comes back before the next call, and the one
     * after sign-in only after the next sign-in.
     */
    fun notNow() {
        prefs.recordingNoticeOffered = VERSION
        _request.value = null
    }

    /** The screen that asked went away (its activity was recreated, say): its action can't run. */
    fun withdraw(owner: Any) {
        if (_request.value?.owner === owner) _request.value = null
    }

    /** Drops the request when its action is no longer possible (the ring stopped, the call ended). */
    fun dropIf(stale: (Action) -> Boolean) {
        val request = _request.value ?: return
        if (stale(request.action)) _request.value = null
    }

    /** Signing out: the next sign-in shows the notice again, unless this device agreed. */
    fun signedOut() {
        prefs.recordingNoticeOffered = 0
        _request.value = null
    }

    companion object {
        /** Raise this when the notice's wording changes enough that people should agree again. */
        const val VERSION = 1
    }
}
