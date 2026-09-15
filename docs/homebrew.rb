class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "2.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-macos-arm64.tar.gz"
      sha256 "1ec0a112717be37319f18186ab8bf98db4fc99a889d19e7ec5f96dfe7591d2be"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-macos-x86_64.tar.gz"
      sha256 "e453d2e5a62bbff3880867e681ad55753debdb8b5454203bfa42fd062873cd61"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-linux-aarch64.tar.gz"
      sha256 "7b1df3d0b8df12f2cd899c5b6e478069b5f64627af2760ffa1ccbe9d4f881712"
    else
      url "https://github.com/bnelabs/r105/releases/download/v2.1.0/r105-linux-x86_64.tar.gz"
      sha256 "fbc04942166baf9682de8bc52fd48cfc4fdffe99f2dbae644c2504d702fd4ac8"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
