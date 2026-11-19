# The E2EE module reaches its Rust library through JNA (uniffi bindings).
# JNA finds its classes, fields and callbacks by name at run time, so
# shrinking must keep them; it also mentions desktop-Java (AWT) classes
# Android doesn't have, which are never used here.
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class uniffi.dragent_e2ee_ffi.** { *; }
-keep class dev.dragent.e2ee.** { *; }
