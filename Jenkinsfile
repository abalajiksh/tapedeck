pipeline {
  agent any // Removed 'docker', runs directly on the agent

  options {
    timestamps()
    disableConcurrentBuilds()
    // Removed 'ansiColor' to fix the second error
  }

  triggers {
    pollSCM('H/2 * * * *')
  }

  environment {
    // Force color output even without ansiColor plugin (logs might look raw, but usually fine)
    CARGO_TERM_COLOR = 'always'
    // Ensure cargo is in PATH if not already (adjust path if needed, e.g., /home/jenkins/.cargo/bin)
    PATH = "$HOME/.cargo/bin:$PATH"
  }

  stages {
    stage('Check Rust') {
      steps {
        sh 'cargo --version'
        sh 'rustc --version'
      }
    }

    stage('Checkout') {
      steps {
        checkout scm
      }
    }

    stage('Release') {
      steps {
        sh 'cargo build --release'
      }
    }

    stage('Archive') {
      steps {
        // Change 'tapedeck' to your actual binary name if different
        archiveArtifacts artifacts: 'target/release/tapedeck', fingerprint: true, allowEmptyArchive: true
      }
    }
  }
}
