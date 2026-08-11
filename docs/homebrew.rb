class R105 < Formula
  include Language::Python::Virtualenv
  desc "r105 — Beyond the prompt. Rich terminal AI assistant for any OpenAI-compatible backend."
  homepage "https://github.com/bnelabs/r105"
  url "https://files.pythonhosted.org/packages/source/r/r105/r105-0.4.1.tar.gz"
  sha256 "ef8b4a9b350683ed563a1d3cc47e5afdba8b7be283b710f83c8439a4320e50f3"
  license "MIT"
  depends_on "python@3.12"
  def install
    virtualenv_install_with_resources
  end
  test do
    assert_match "r105 #{version}", shell_output("#{bin}/r105 --version")
  end
end
