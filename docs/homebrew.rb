class R105 < Formula
  include Language::Python::Virtualenv
  desc "r105 — Beyond the prompt. Rich terminal AI assistant for any OpenAI-compatible backend."
  homepage "https://github.com/bnelabs/r105"
  url "https://files.pythonhosted.org/packages/source/r/r105/r105-0.6.0.tar.gz"
  sha256 "17997b2cc89c8e4271818eed0189eeb1c3d8491c9e71d8ec3c2662a58f666882"
  license "MIT"
  depends_on "python@3.12"
  def install
    virtualenv_install_with_resources
  end
  test do
    assert_match "r105 #{version}", shell_output("#{bin}/r105 --version")
  end
end
