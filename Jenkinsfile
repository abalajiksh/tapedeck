pipeline {
  agent any

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
    // Keep only the last 10 builds to save space
    buildDiscarder(logRotator(numToKeepStr: '10'))
  }

  triggers {
    pollSCM('H/2 * * * *')
  }

  environment {
    CARGO_TERM_COLOR = 'always'
    PATH = "$HOME/.cargo/bin:$PATH"
    DEPLOY_HOST = credentials('tapedeck-lxc-ip')
    DEPLOY_DIR = credentials('tapedeck-lxc-dir')
    
    // SonarQube configuration
    SONAR_PROJECT_KEY = 'tapedeck'
    SONAR_PROJECT_NAME = 'Tapedeck'
    SONAR_SOURCES = 'src'
    SONARQUBE_URL = "${env.SONARQUBE_URL ?: 'http://192.168.178.101:9000'}"
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
            def scannerHome = tool name: 'SonarQube Scanner', type: 'hudson.plugins.sonar.SonarRunnerInstallation'
            
            withCredentials([string(credentialsId: 'sonarqube-token', variable: 'SONAR_TOKEN')]) {
              echo "=========================================="
              echo "Running SonarQube Analysis"
              echo "=========================================="
              
              sh """
                "${scannerHome}/bin/sonar-scanner" \
                  -Dsonar.projectKey=${SONAR_PROJECT_KEY} \
                  -Dsonar.projectName="${SONAR_PROJECT_NAME}" \
                  -Dsonar.sources=${SONAR_SOURCES} \
                  -Dsonar.host.url=${SONARQUBE_URL} \
                  -Dsonar.login=\$SONAR_TOKEN
              """
            }
          } catch (Exception e) {
            echo "⚠ SonarQube analysis failed: ${e.message}"
            echo "  Verify: 'sonarqube-token' credential is valid"
          }
        }
      }
    }

    stage('Deploy') {
      steps {
        script {
          // Use username/password credential for simpler auth
          withCredentials([usernamePassword(credentialsId: 'tapedeck-lxc-auth', usernameVariable: 'LXC_USER', passwordVariable: 'LXC_PASS')]) {
            sh """
              echo "=========================================="
              echo "Deploying to \$DEPLOY_HOST"
              echo "=========================================="
              
              echo "Debug: Checking Environment"
              echo "PATH is: \$PATH"
              echo "sshpass location: \$(command -v sshpass)"
              sshpass -V || echo "sshpass version check failed"
              
              # Define common SSH options
              # StrictHostKeyChecking=no is used to avoid interactive prompt for new hosts
              SSH_OPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null"
              
              # Test SSH connection
              echo "Testing SSH connection to \$LXC_USER@\$DEPLOY_HOST..."
              sshpass -p "\$LXC_PASS" ssh \$SSH_OPTS "\$LXC_USER@\$DEPLOY_HOST" 'echo "SSH connection successful"'
              
              # Copy binary to target server
              echo "Copying binary..."
              sshpass -p "\$LXC_PASS" scp \$SSH_OPTS target/release/tapedeck "\$LXC_USER@\$DEPLOY_HOST:\$DEPLOY_DIR/"
              
              # Make executable and restart service
              echo "Restarting service..."
              sshpass -p "\$LXC_PASS" ssh \$SSH_OPTS "\$LXC_USER@\$DEPLOY_HOST" \
                "chmod +x \$DEPLOY_DIR/tapedeck && \
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
          def version = sh(
            script: "cargo pkgid | cut -d# -f2 | cut -d@ -f2",
            returnStdout: true
          ).trim()

          def commitSha = sh(
            script: 'git rev-parse --short HEAD',
            returnStdout: true
          ).trim()

          echo "Detected version: ${version}"
          echo "Commit SHA: ${commitSha}"

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
