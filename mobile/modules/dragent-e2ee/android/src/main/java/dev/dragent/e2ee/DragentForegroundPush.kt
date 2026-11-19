package dev.dragent.e2ee

/**
 * A decrypted push that arrives while the app is open goes to the app's own
 * banner instead of the system's. The module sets [listener] while the app's
 * JavaScript is running; the push service calls [deliver].
 */
object DragentForegroundPush {
  @Volatile
  var listener: ((title: String, body: String, desktop: String?) -> Unit)? = null

  /** True when the app took it (and the service needn't pop it up itself). */
  fun deliver(title: String, body: String, desktop: String?): Boolean {
    val current = listener ?: return false
    return try {
      current(title, body, desktop)
      true
    } catch (_: Throwable) {
      false
    }
  }
}
