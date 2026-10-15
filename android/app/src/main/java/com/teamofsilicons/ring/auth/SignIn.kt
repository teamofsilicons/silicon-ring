package com.teamofsilicons.ring.auth

import android.content.Context
import android.net.Uri
import androidx.browser.customtabs.CustomTabColorSchemeParams
import androidx.browser.customtabs.CustomTabsIntent
import androidx.core.graphics.toColorInt
import com.teamofsilicons.ring.BuildConfig
import com.teamofsilicons.ring.data.ApiException
import com.teamofsilicons.ring.data.AppPrefs
import com.teamofsilicons.ring.data.CarbonLogin
import com.teamofsilicons.ring.data.RingApi
import com.teamofsilicons.ring.data.Session
import kotlinx.coroutines.flow.MutableStateFlow
import java.security.MessageDigest
import java.security.SecureRandom
import android.util.Base64

/**
 * Hosted Silicon Accounts sign-in for carbons (udd/accounts.md): PKCE S256 in a Custom Tab, the
 * `com.teamofsilicons.ring:/auth/callback` redirect, then `POST /v1/auth/carbon`.
 */
class SignIn(private val api: RingApi, private val prefs: AppPrefs) {

    // Progress for the sign-in screen. It lives here rather than in the activity so a code
    // exchange in flight survives the activity being recreated (each code is single use).
    val busy = MutableStateFlow(false)
    val message = MutableStateFlow<String?>(null)

    /** Opens the Accounts sign-in page. Returns an error message, or null when the page opened. */
    suspend fun start(context: Context): String? {
        val apiUrl = prefs.apiUrl.value
        val accountsUrl = try {
            api.meta(apiUrl).accountsUrl
        } catch (e: ApiException) {
            if (e.isNetwork) return "Can't reach Ring at $apiUrl."
            BuildConfig.ACCOUNTS_URL
        }
        val verifier = randomToken(48)
        val state = randomToken(24)
        prefs.savePendingSignIn(state, verifier, apiUrl)
        val url = Uri.parse(accountsUrl.trimEnd('/') + "/authorize").buildUpon()
            .appendQueryParameter("response_type", "code")
            .appendQueryParameter("app_id", "ring")
            .appendQueryParameter("redirect_uri", REDIRECT_URI)
            .appendQueryParameter("scope", "profile")
            .appendQueryParameter("state", state)
            .appendQueryParameter("code_challenge", challenge(verifier))
            .appendQueryParameter("code_challenge_method", "S256")
            .build()
        val colors = CustomTabColorSchemeParams.Builder()
            .setToolbarColor("#FFFDF9".toColorInt())
            .setNavigationBarColor("#FFFDF9".toColorInt())
            .build()
        val darkColors = CustomTabColorSchemeParams.Builder()
            .setToolbarColor("#181818".toColorInt())
            .setNavigationBarColor("#181818".toColorInt())
            .build()
        return try {
            CustomTabsIntent.Builder()
                .setDefaultColorSchemeParams(colors)
                .setColorSchemeParams(CustomTabsIntent.COLOR_SCHEME_DARK, darkColors)
                .setShowTitle(true)
                .setShareState(CustomTabsIntent.SHARE_STATE_OFF)
                .build()
                .launchUrl(context, url)
            null
        } catch (e: android.content.ActivityNotFoundException) {
            "Install a web browser to sign in with Silicon Accounts."
        }
    }

    sealed interface Result {
        data class Success(val apiUrl: String, val session: Session) : Result
        data class Failure(val message: String) : Result
        data object Cancelled : Result
        data object NotOurs : Result
    }

    /** Completes sign-in from the redirect [uri]. */
    suspend fun complete(uri: Uri, label: String): Result {
        if (uri.scheme != SCHEME || uri.path != "/auth/callback") return Result.NotOurs
        val pending = prefs.pendingSignIn() ?: return Result.Failure("This sign-in link has expired. Try again.")
        val (expectedState, verifier, apiUrl) = pending
        if (uri.getQueryParameter("state") != expectedState) {
            return Result.Failure("The sign-in response did not match. Try again.")
        }
        // Each code is single use: drop the pending state before exchanging it.
        prefs.clearPendingSignIn()
        uri.getQueryParameter("error")?.let { error ->
            return if (error == "access_denied") Result.Cancelled
            else Result.Failure(uri.getQueryParameter("error_description") ?: "Sign-in failed ($error).")
        }
        val code = uri.getQueryParameter("code") ?: return Result.Failure("Silicon Accounts did not return a code.")
        return try {
            val session = api.signInCarbon(apiUrl, CarbonLogin(code, verifier, REDIRECT_URI, label))
            Result.Success(apiUrl, session)
        } catch (e: ApiException) {
            Result.Failure(
                when (e.code) {
                    "silicon_use_cli" -> "Silicons use the ring CLI. Sign in with a carbon account here."
                    else -> e.userMessage
                },
            )
        }
    }

    private fun randomToken(bytes: Int): String {
        val buffer = ByteArray(bytes).also { SecureRandom().nextBytes(it) }
        return Base64.encodeToString(buffer, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    }

    private fun challenge(verifier: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII))
        return Base64.encodeToString(digest, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    }

    companion object {
        const val SCHEME = "com.teamofsilicons.ring"
        const val REDIRECT_URI = "com.teamofsilicons.ring:/auth/callback"
    }
}
