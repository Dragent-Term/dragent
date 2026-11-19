import Link from "next/link";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
export default function NotFound() {
  return (
    <>
      <Nav />
      <main id="main-content" className="not-found container-wide">
        <p className="eyebrow">404 / Page not found</p>
        <h1>
          Let’s get you
          <br />
          back to the work.
        </h1>
        <p>
          This page may have moved. Find a guide in the documentation or return
          to the product.
        </p>
        <div className="hero-actions">
          <Link href="/docs" className="button">
            Browse documentation
          </Link>
          <Link href="/" className="button button-secondary">
            Back home
          </Link>
        </div>
      </main>
      <Footer />
    </>
  );
}
