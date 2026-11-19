import { redirect } from "next/navigation";

// Bare domain → the billing dashboard, which handles web sign-in when there's
// no session. The desktop app opens the explicit /login/desktop?state=… URL,
// so its linking flow is unaffected by this default.
export default function Home() {
  redirect("/dashboard");
}
