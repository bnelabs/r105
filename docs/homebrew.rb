class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.2.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.2.0/r105-macos-arm64.tar.gz"
      sha256 "e543c28adaf195ec91d2b9aa6f4c637cbfb12c9f984ac37bfb63b9f60534bb9b"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.2.0/r105-macos-x86_64.tar.gz"
      sha256 "4a70ab5b51da9bf45c4f176397d65d37e56148af13ea2d9e84d29b4bb1f12a6d"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.2.0/r105-linux-aarch64.tar.gz"
      sha256 "4d5cb7a9c79e45f2cc7b4688743b08b7449d040cd8dfa990c5f11e020e0a54d3"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.2.0/r105-linux-x86_64.tar.gz"
      sha256 "c0ae25ca998c51cd3540468bc35bc3695ccbf02eae169d0b1f9a91b2e7f303fc"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
