package com.teamofsilicons.ring.data

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.Serializable
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** A signed-in Ring session. The token is bound to the API origin it was issued by. */
@Serializable
data class StoredSession(
    val apiUrl: String,
    val token: String,
    val expiresAt: String,
    val account: Account,
    val deviceId: String,
)

/**
 * Keeps the Ring session token sealed with a non-exportable AES-GCM key in the Android Keystore.
 * The ciphertext lives in private shared preferences; nothing here is ever backed up.
 */
class SessionStore(context: Context) {
    private val prefs = context.getSharedPreferences("ring-session", Context.MODE_PRIVATE)
    private val _session = MutableStateFlow(load())
    val session: StateFlow<StoredSession?> = _session.asStateFlow()

    val current: StoredSession? get() = _session.value

    fun save(value: StoredSession) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.doFinal(RingJson.encodeToString(StoredSession.serializer(), value).toByteArray(Charsets.UTF_8))
        prefs.edit(commit = true) {
            putString(KEY_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            putString(KEY_SESSION, Base64.encodeToString(sealed, Base64.NO_WRAP))
        }
        _session.value = value
    }

    /** Refreshes the cached account (display name and photo change on Accounts). */
    fun updateAccount(account: Account) {
        val existing = _session.value ?: return
        if (existing.account != account) save(existing.copy(account = account))
    }

    fun clear() {
        prefs.edit(commit = true) { clear() }
        _session.value = null
    }

    private fun load(): StoredSession? {
        val sealed = prefs.getString(KEY_SESSION, null) ?: return null
        val iv = prefs.getString(KEY_IV, null) ?: return null
        return try {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)))
            val plain = cipher.doFinal(Base64.decode(sealed, Base64.NO_WRAP)).toString(Charsets.UTF_8)
            RingJson.decodeFromString(StoredSession.serializer(), plain)
        } catch (e: Exception) {
            // A lost Keystore key (for example after a restore) leaves an unreadable blob: start over.
            Log.w(TAG, "Stored session could not be opened; signing out", e)
            prefs.edit(commit = true) { clear() }
            null
        }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val TAG = "RingSession"
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "ring-session"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val KEY_IV = "iv"
        const val KEY_SESSION = "session"
    }
}
