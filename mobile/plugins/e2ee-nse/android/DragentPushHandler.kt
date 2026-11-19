// What the Android messaging service does with one push
// (docs/design/e2ee.md §8 step 5 and step 6). No Android UI here.
//
// - The sealed blob is the data message's "e" string (1,494 base64url chars).
// - DragentE2EEPush.open decrypts with the Keystore-wrapped push key(s) and,
//   in Rust, looks the sender's static key up in the push-pin list, checks the
//   inner desktop_id and the 24-hour limit.
// - Duplicate ids are dropped with a seen-id list (not secret).
// - A dismissal (dismiss = 1) cancels the listed notifications by tag.
// - ANY failure posts the constant placeholder from the app's resources,
//   never text from the payload.
package dev.dragent.e2ee.push

import android.content.Context
import android.util.Base64
import org.json.JSONObject
import java.security.MessageDigest

sealed class DragentPushOutcome {
  /** Show a notification with tag = hex id (id 0) in group = thread. */
  data class Show(
    val title: String,
    val body: String,
    val tag: String,
    val group: String?,
    val desktop: String,
    /** An agent's pane waiting on a permission prompt: Allow / Deny buttons. */
    val pane: String? = null,
  ) : DragentPushOutcome()
  /** Cancel these tags; nothing is shown. */
  data class Dismiss(val tags: List<String>) : DragentPushOutcome()
  /** The constant placeholder. `why` is for tests only. */
  data class Placeholder(val why: String) : DragentPushOutcome()
}

/** What the handler needs from a decrypted push (uniffi's PushNotification). */
data class DragentOpenedPush(
  val desktopId: String,
  val id: ByteArray,
  val issuedAtMs: Long,
  val dismiss: Boolean,
  val title: String,
  val body: String,
  val workspaceId: String,
  val dismissIds: List<ByteArray>,
  /** 2 = an agent needs you. */
  val kind: Int = 0,
  /** The agent's pane it's about (all zeros when none). */
  val tabId: String = "",
)

/** Seen ids (hex → issued_at_ms) kept 24 hours, in plain SharedPreferences. */
class DragentSeenIds(private val context: Context?) {
  private val mem = HashMap<String, Long>()

  private fun load(): MutableMap<String, Long> {
    val ctx = context ?: return mem
    val raw = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, null) ?: return HashMap()
    return try {
      val o = JSONObject(raw)
      val out = HashMap<String, Long>()
      for (k in o.keys()) out[k] = o.getLong(k)
      out
    } catch (_: Exception) {
      HashMap()
    }
  }

  private fun save(map: Map<String, Long>) {
    val ctx = context ?: return
    val o = JSONObject()
    for ((k, v) in map) o.put(k, v)
    ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, o.toString()).apply()
  }

  /** False when `id` was seen in the last 24 hours. */
  @Synchronized
  fun insert(id: String, issuedAtMs: Long, nowMs: Long): Boolean {
    val seen = load().filterValues { nowMs < it || nowMs - it < KEEP_MS }.toMutableMap()
    if (seen.containsKey(id)) return false
    seen[id] = issuedAtMs
    if (seen.size > MAX) {
      seen.entries.sortedBy { it.value }.take(seen.size - MAX).forEach { seen.remove(it.key) }
    }
    if (context == null) { mem.clear(); mem.putAll(seen) } else save(seen)
    return true
  }

  companion object {
    const val PREFS = "dragent.push.seen.v1"
    const val KEY = "seen"
    const val KEEP_MS = 24L * 60 * 60 * 1000
    const val MAX = 1024
  }
}

object DragentPushHandler {
  const val BLOB_KEY = "e"
  const val SEALED_LEN = 1120

  private val B64URL = Regex("^[A-Za-z0-9_-]+$")

  fun decodeBase64Url(text: String): ByteArray? {
    if (text.isEmpty() || !B64URL.matches(text) || text.length % 4 == 1) return null
    return try {
      Base64.decode(text, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
    } catch (_: IllegalArgumentException) {
      null
    }
  }

  fun hex(b: ByteArray): String = b.joinToString("") { "%02x".format(it) }

  private fun uuidBytes(s: String): ByteArray? {
    val h = s.replace("-", "")
    if (h.length != 32) return null
    return try { ByteArray(16) { h.substring(it * 2, it * 2 + 2).toInt(16).toByte() } } catch (_: NumberFormatException) { null }
  }

  /** §8: the group is SHA-256(desktop || workspace), hex (same as iOS's thread). */
  fun thread(desktop: String, workspace: String): String? {
    val d = uuidBytes(desktop) ?: return null
    val w = uuidBytes(workspace) ?: return null
    return hex(MessageDigest.getInstance("SHA-256").digest(d + w))
  }

  /** "Needs you" (kind 2) about a real pane (a UUID that isn't all zeros): answerable. */
  /**
   * The desktop titles an agent's notification "<agent> · <workspace>"
   * (agent names as the desktop shows them). The agent's icon, by the name
   * before the " · ", ships as drawable `agent_<key>`; null when the title
   * names no agent the app has an icon for.
   */
  val AGENT_ICONS: Map<String, String> = mapOf(
    "Dragent" to "dragent", "Coordination" to "dragent", "Claude Code" to "claude", "Codex" to "codex", "Gemini CLI" to "gemini",
    "Gemini" to "gemini", "Copilot CLI" to "copilot", "OpenCode" to "opencode", "Amp" to "amp",
    "Droid" to "droid", "Grok CLI" to "grok", "Devin" to "devin", "Kimi" to "kimi", "Kiro (AWS)" to "kiro",
    "Pi" to "pi", "Oh My Pi" to "pi", "Mistral Vibe" to "vibe", "Hermes Agent" to "hermes", "Muse Code" to "muse",
    "Goose" to "goose", "Cline" to "cline", "Codebuff" to "codebuff", "Continue" to "continue",
    "Kilocode" to "kilocode", "Crush" to "crush", "Rovo Dev" to "rovo", "Junie" to "junie", "Antigravity" to "antigravity",
  )

  fun agentIcon(title: String): String? {
    val at = title.indexOf(" · ")
    if (at <= 0) return null
    return AGENT_ICONS[title.substring(0, at)]
  }

  fun answerPane(kind: Int, tab: String): String? {
    val hex = tab.replace("-", "").lowercase()
    if (kind != 2 || hex.length != 32 || !hex.all { it in '0'..'9' || it in 'a'..'f' } || hex.all { it == '0' }) return null
    return tab.lowercase()
  }

  fun handle(
    data: Map<String, String>,
    nowMs: Long,
    seen: DragentSeenIds,
    open: (ByteArray, Long) -> DragentOpenedPush,
  ): DragentPushOutcome {
    val text = data[BLOB_KEY] ?: return DragentPushOutcome.Placeholder("missing blob")
    val sealed = decodeBase64Url(text)
    if (sealed == null || sealed.size != SEALED_LEN) return DragentPushOutcome.Placeholder("bad encoding")
    val n = try {
      open(sealed, nowMs)
    } catch (_: Throwable) {
      return DragentPushOutcome.Placeholder("open failed")
    }
    val id = hex(n.id)
    if (!seen.insert(id, n.issuedAtMs, nowMs)) return DragentPushOutcome.Placeholder("duplicate")
    if (n.dismiss) return DragentPushOutcome.Dismiss(n.dismissIds.map { hex(it) })
    return DragentPushOutcome.Show(
      title = n.title.ifEmpty { "Dragent" },
      body = n.body,
      tag = id,
      group = thread(n.desktopId, n.workspaceId),
      desktop = n.desktopId,
      pane = answerPane(n.kind, n.tabId),
    )
  }
}
