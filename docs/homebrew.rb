class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.0.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.0/r105-macos-arm64.tar.gz"
      sha256 "7cea43e36e24fae2e1b562761851e9a216cb23fade3dbc74ea830249d28d29e9"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.0/r105-macos-x86_64.tar.gz"
      sha256 "22dd1e6045e88bf23d6024714150c71e23dbf2f0da523ad9a34875c1f79cdc2c"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.0/r105-linux-aarch64.tar.gz"
      sha256 "c52b6d5616ea58d5f94741b16d198d0b6bbe797c15bdd1321d2a187a7ebf0274"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.0/r105-linux-x86_64.tar.gz"
      sha256 "9483cfcfc0f73822fafddccacf942bd6207faec32f27387a1e6997747d749168"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
