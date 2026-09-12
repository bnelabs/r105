class R105 < Formula
  desc "Native local-first AI harness for OpenAI-compatible backends"
  homepage "https://github.com/bnelabs/r105"
  version "1.0.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.1/r105-macos-arm64.tar.gz"
      sha256 "e63ce475111e3de0b6edb76f1ab3e7532166fa6c34256a7ab6abd7d785483377"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.1/r105-macos-x86_64.tar.gz"
      sha256 "99c2b6ac87121e144d385197ebafe976c08fbf9fc15e692a787b7fe15509ddd6"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/bnelabs/r105/releases/download/v1.0.1/r105-linux-aarch64.tar.gz"
      sha256 "63f8f51faef3326d69063b5027e21072ec72555a672e33ccde67ba3e3d4bb4f9"
    else
      url "https://github.com/bnelabs/r105/releases/download/v1.0.1/r105-linux-x86_64.tar.gz"
      sha256 "ea4fbe98c7fbdb319101b1ebb3d0b5f9ed261ec2945b2986e4d259aea0702595"
    end
  end

  def install
    bin.install "r105"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/r105 --version")
  end
end
