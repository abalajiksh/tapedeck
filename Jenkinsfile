pipeline {
  agent any

  tools {
    // Reference the SonarQube Scanner installed via Jenkins plugin
    // Make sure this matches the name in Jenkins Global Tool Configuration
    // Default name is usually 'SonarQube Scanner' but verify in:
    // Manage Jenkins → Global Tool Configuration → SonarQube Scanner
    'hudson.plugins.sonar.SonarRunnerInstallation' 'SonarQube Scanner'
  }

  parameters {
    string(
      name: 'COMMIT_ID',
      defaultValue: 'main',
      description: 'Git commit ID, branch name, or tag to build (default: main)'
    )
    booleanParam(
      name: 'SKIP_SONARQUBE',
      defaultValue: false,
      description: 'Skip SonarQube analysis'
    )
  }

  options {
    timestamps()
    disableConcurrentBuilds()
  }

  triggers {
    pollSCM('H/2 * * * *')
  }

  environment {
    CARGO_TERM_COLOR = 'always'
    PATH = "$HOME/.cargo/bin:$PATH"
    DEPLOY_HOST = credentials('tapedeck-lxc-ip')
    DEPLOY_DIR = credentials('tapedeck-lxc-dir')
    DEPLOY_USER = credentials('tapedeck-deploy-user')
    
    // SonarQube configuration
    SONAR_PROJECT_KEY = 'tapedeck'
    SONAR_PROJECT_NAME = 'Tapedeck'
    SONAR_SOURCES = 'src'
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
        script {
          echo "Building commit/branch: ${params.COMMIT_ID}"
          checkout([
            $class: 'GitSCM',
            branches: [[name: "${params.COMMIT_ID}"]],
            userRemoteConfigs: scm.userRemoteConfigs
          ])
        }
      }
    }

    stage('Clean') {
      steps {
        sh 'cargo clean'
      }
    }

    stage('Release') {
      steps {
        sh 'cargo build --release'
      }
    }

    stage('SonarQube Analysis') {
      when {
        expression { return !params.SKIP_SONARQUBE }
      }
      steps {
        script {
          try {
            // Use withSonarQubeEnv to leverage Jenkins SonarQube plugin
            // 'SonarQube' should match the server name in Jenkins System Configuration
            withSonarQubeEnv('SonarQube') {
              echo "=========================================="
              echo "Running SonarQube Analysis"
              echo "=========================================="
              
              sh """
                sonar-scanner \
                  -Dsonar.projectKey=${SONAR_PROJECT_KEY} \
                  -Dsonar.projectName="${SONAR_PROJECT_NAME}" \
                  -Dsonar.sources=${SONAR_SOURCES}
              """
            }
          } catch (Exception e) {
            echo "⚠ SonarQube analysis failed: ${e.message}"
            echo "  Verify: SonarQube server configured in Jenkins System settings"
            echo "  Verify: SonarQube Scanner tool installed in Global Tool Configuration"
          }
        }
      }
    }

    stage('Deploy') {
      steps {
        script {
          // Use sshagent instead of manual key handling to avoid libcrypto issues
          sshagent(credentials: ['tapedeck-ssh-key']) {
            sh """
              echo "=========================================="
              echo "Deploying to ${DEPLOY_HOST}"
              echo "=========================================="
              
              # Test SSH connection
              echo "Testing SSH connection..."
              ssh -o StrictHostKeyChecking=no ${DEPLOY_USER}@${DEPLOY_HOST} 'echo "SSH connection successful"'
              
              # Copy binary to target server
              echo "Copying binary..."
              scp -o StrictHostKeyChecking=no target/release/tapedeck ${DEPLOY_USER}@${DEPLOY_HOST}:${DEPLOY_DIR}/
              
              # Make executable and restart service
              echo "Restarting service..."
              ssh -o StrictHostKeyChecking=no ${DEPLOY_USER}@${DEPLOY_HOST} \
                "chmod +x ${DEPLOY_DIR}/tapedeck && \
                 systemctl restart tapedeck && \
                 systemctl status tapedeck --no-pager"
              
              echo "Deployment completed successfully!"
            """
          }
        }
      }
    }

    stage('Archive') {
      steps {
        script {
          // cargo pkgid returns a URL-like string ending in #name:version
          // We use 'cut' to extract just the version part after the last colon
          def version = sh(
            script: "cargo pkgid | cut -d# -f2 | cut -d@ -f2",
            returnStdout: true
          ).trim()

          // Get short commit SHA for artifact naming
          def commitSha = sh(
            script: 'git rev-parse --short HEAD',
            returnStdout: true
          ).trim()

          echo "Detected version: ${version}"
          echo "Commit SHA: ${commitSha}"

          // Copy and archive with version and commit info
          sh "cp target/release/tapedeck target/release/tapedeck-${version}-${commitSha}"
          archiveArtifacts artifacts: "target/release/tapedeck-${version}-${commitSha}", fingerprint: true
        }
      }
    }
  }

  post {
    always {
      echo "Build completed for commit: ${params.COMMIT_ID}"
    }
    success {
      echo 'Build and deployment completed successfully!'
    }
    failure {
      echo 'Build or deployment failed!'
    }
  }
}
