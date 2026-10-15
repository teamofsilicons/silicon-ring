package com.teamofsilicons.ring.ui

import com.teamofsilicons.ring.R
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.data.Call
import com.teamofsilicons.ring.data.CallState
import com.teamofsilicons.ring.data.InvitationState
import com.teamofsilicons.ring.data.parseInstant
import java.time.Duration
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

object Format {
    private val time = DateTimeFormatter.ofPattern("HH:mm", Locale.getDefault())
    private val weekday = DateTimeFormatter.ofPattern("EEE", Locale.getDefault())
    private val day = DateTimeFormatter.ofPattern("d MMM", Locale.getDefault())
    private val dayYear = DateTimeFormatter.ofPattern("d MMM yyyy", Locale.getDefault())
    private val full = DateTimeFormatter.ofPattern("d MMM yyyy, HH:mm", Locale.getDefault())

    /** "TODAY 14:02", "YESTERDAY 09:10", "MON 18:00", "12 OCT". */
    fun whenShort(iso: String?): String {
        val instant = parseInstant(iso) ?: return ""
        val zone = ZoneId.systemDefault()
        val date = instant.atZone(zone).toLocalDate()
        val today = LocalDate.now(zone)
        val clock = time.format(instant.atZone(zone))
        return when {
            date == today -> "Today $clock"
            date == today.minusDays(1) -> "Yesterday $clock"
            date.isAfter(today.minusDays(7)) -> "${weekday.format(date)} $clock"
            date.year == today.year -> day.format(date)
            else -> dayYear.format(date)
        }
    }

    fun whenFull(iso: String?): String = parseInstant(iso)?.let { full.format(it.atZone(ZoneId.systemDefault())) } ?: ""

    fun clock(iso: String?): String = parseInstant(iso)?.let { time.format(it.atZone(ZoneId.systemDefault())) } ?: ""

    /** "0:42", "12:05", "1:02:03". */
    fun duration(ms: Long): String {
        val total = (ms / 1000).coerceAtLeast(0)
        val h = total / 3600
        val m = (total % 3600) / 60
        val s = total % 60
        return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
    }

    fun elapsed(since: Instant?, now: Instant = Instant.now()): String =
        since?.let { duration(Duration.between(it, now).toMillis()) } ?: "0:00"

    fun callDurationMs(call: Call): Long? {
        val start = parseInstant(call.answeredAt) ?: return null
        val end = parseInstant(call.endedAt) ?: return null
        return Duration.between(start, end).toMillis()
    }

    /** How a call turned out, from the viewer's side. */
    data class Outcome(val label: String, val missed: Boolean, val icon: Int)

    fun outcome(call: Call, me: String): Outcome {
        val mine = call.invitations.lastOrNull { it.to.uuid == me }
        val outgoing = call.createdBy.uuid == me
        val answered = call.answeredAt != null
        val duration = callDurationMs(call)
        if (call.state == CallState.ACTIVE) return Outcome("On call", false, if (outgoing) R.drawable.ms_call_made else R.drawable.ms_call_received)
        if (call.state == CallState.RINGING) return Outcome(if (outgoing) "Calling" else "Ringing", false, if (outgoing) R.drawable.ms_call_made else R.drawable.ms_call_received)
        if (outgoing) {
            val theirs = call.invitations.firstOrNull { it.from.uuid == me }
            return when {
                answered && duration != null -> Outcome(duration(duration), false, R.drawable.ms_call_made)
                answered -> Outcome("Answered", false, R.drawable.ms_call_made)
                call.voicemailId != null -> Outcome("Voicemail", false, R.drawable.ms_call_missed_outgoing)
                theirs?.state == InvitationState.DECLINED -> Outcome("Declined", false, R.drawable.ms_call_missed_outgoing)
                theirs?.state == InvitationState.BUSY -> Outcome("Busy", false, R.drawable.ms_call_missed_outgoing)
                call.endReason == "cancelled" -> Outcome("Cancelled", false, R.drawable.ms_call_missed_outgoing)
                else -> Outcome("No answer", false, R.drawable.ms_call_missed_outgoing)
            }
        }
        return when (mine?.state) {
            InvitationState.ACCEPTED -> Outcome(duration?.let { duration(it) } ?: "Answered", false, R.drawable.ms_call_received)
            InvitationState.DECLINED -> Outcome("Declined", false, R.drawable.ms_call_received)
            InvitationState.BUSY -> Outcome("Missed · busy", true, R.drawable.ms_call_missed)
            InvitationState.MISSED, InvitationState.CANCELLED -> Outcome("Missed", true, R.drawable.ms_call_missed)
            else -> if (answered) Outcome(duration?.let { duration(it) } ?: "Answered", false, R.drawable.ms_call_received)
            else Outcome("Missed", true, R.drawable.ms_call_missed)
        }
    }

    /** The people a call is "with", for list rows. */
    fun counterpart(call: Call, me: String): List<Account> = call.others(me).ifEmpty { listOf(call.createdBy) }

    fun voicemailReason(reason: String, declineReason: String?): String = when (reason) {
        "declined" -> declineReason?.takeIf { it.isNotBlank() }?.let { "Declined · $it" } ?: "Declined"
        "busy" -> "Busy"
        else -> "Missed"
    }
}
