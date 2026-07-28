use super::*;

fn test_config() -> RunnerConfig {
    RunnerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        docker_bin: "docker".into(),
        allowed_roots: vec!["/home/sulion/repos".into()],
        default_memory: Some("1g".into()),
        default_cpus: Some("1".into()),
        default_pids_limit: Some("128".into()),
    }
}

fn prepare(request: &DockerCommandRequest) -> Result<PreparedDockerArgs, RunnerError> {
    prepare_docker_args(request, &test_config(), Path::new("/home/sulion/repos/app"))
}

#[test]
fn run_injects_labels_and_limits() {
    let request = DockerCommandRequest {
        pty_id: Some("pty-1".into()),
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["run".into(), "--rm".into(), "alpine".into(), "true".into()],
    };
    let prepared = prepare(&request).unwrap();
    assert_eq!(prepared.subcommand, DockerSubcommand::Run);
    assert!(prepared
        .args
        .contains(&format!("{OWNER_LABEL}={OWNER_VALUE}")));
    assert!(prepared.args.contains(&format!("{PTY_LABEL}=pty-1")));
    assert!(prepared
        .args
        .windows(2)
        .any(|pair| pair == ["--memory", "1g"]));
    assert!(prepared.args.windows(2).any(|pair| pair == ["--cpus", "1"]));
    assert!(prepared
        .args
        .windows(2)
        .any(|pair| pair == ["--network", "sulion"]));
    assert!(prepared
        .args
        .windows(2)
        .any(|pair| pair == ["--pids-limit", "128"]));
}

#[test]
fn run_denies_privileged_and_mounts() {
    for args in [
        vec!["run", "--privileged", "alpine"],
        vec!["run", "-v", "/:/host", "alpine"],
        vec!["run", "-v/:/host", "alpine"],
        vec!["run", "--mount", "type=bind,src=/,dst=/host", "alpine"],
        vec!["run", "--network=host", "alpine"],
        vec!["run", "--network=default", "alpine"],
        vec!["run", "--network", "none", "alpine"],
        vec!["run", "-it", "alpine"],
    ] {
        let request = DockerCommandRequest {
            pty_id: None,
            cwd: "/home/sulion/repos/app".into(),
            argv: args.into_iter().map(str::to_string).collect(),
        };
        assert!(prepare(&request).is_err());
    }
}

#[test]
fn run_denies_resource_limit_overrides() {
    for args in [
        vec!["run", "--memory", "10g", "alpine"],
        vec!["run", "-m10g", "alpine"],
        vec!["run", "--cpus=32", "alpine"],
        vec!["run", "--pids-limit", "-1", "alpine"],
        vec!["run", "--restart=always", "alpine"],
    ] {
        let request = DockerCommandRequest {
            pty_id: None,
            cwd: "/home/sulion/repos/app".into(),
            argv: args.into_iter().map(str::to_string).collect(),
        };
        assert!(prepare(&request).is_err());
    }
}

#[test]
fn build_injects_labels_and_preserves_context() {
    let request = DockerCommandRequest {
        pty_id: Some("pty-2".into()),
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["build".into(), "-t".into(), "local/app".into(), ".".into()],
    };
    let prepared = prepare(&request).unwrap();
    assert_eq!(prepared.subcommand, DockerSubcommand::Build);
    assert!(prepared
        .args
        .contains(&format!("{OWNER_LABEL}={OWNER_VALUE}")));
    assert!(prepared.args.contains(&".".to_string()));
}

#[test]
fn build_denies_host_output_flags() {
    for args in [
        vec!["build", "--output", "type=local,dest=/tmp/out", "."],
        vec!["build", "-o", "/tmp/out", "."],
        vec!["build", "--iidfile=/tmp/iid", "."],
    ] {
        let request = DockerCommandRequest {
            pty_id: None,
            cwd: "/home/sulion/repos/app".into(),
            argv: args.into_iter().map(str::to_string).collect(),
        };
        assert!(prepare(&request).is_err());
    }
}

#[test]
fn logs_denies_follow_mode() {
    let request = DockerCommandRequest {
        pty_id: None,
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["logs".into(), "--follow".into(), "container".into()],
    };
    assert!(prepare(&request).is_err());
}

#[test]
fn ps_is_label_filtered() {
    let request = DockerCommandRequest {
        pty_id: None,
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["ps".into(), "-a".into()],
    };
    let prepared = prepare(&request).unwrap();
    assert_eq!(
        prepared.args,
        vec!["ps", "--filter", "label=sulion.owner=sulion", "-a"]
    );
}

#[test]
fn compose_uses_sulion_network_override() {
    let cwd = std::env::temp_dir().join(format!("sulion-compose-test-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(
        cwd.join("compose.yaml"),
        "services:\n  app:\n    image: alpine\n",
    )
    .unwrap();

    let request = DockerCommandRequest {
        pty_id: None,
        cwd: cwd.to_string_lossy().into_owned(),
        argv: vec!["compose".into(), "up".into(), "-d".into()],
    };
    let prepared = prepare_docker_args(&request, &test_config(), &cwd).unwrap();
    assert_eq!(prepared.subcommand, DockerSubcommand::Compose);
    assert_eq!(
        prepared.args,
        vec![
            "compose",
            "-f",
            "compose.yaml",
            "-f",
            COMPOSE_NETWORK_OVERRIDE_PATH,
            "up",
            "-d",
        ]
    );

    std::fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn compose_denies_exec() {
    let request = DockerCommandRequest {
        pty_id: None,
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["compose".into(), "exec".into(), "app".into(), "sh".into()],
    };
    assert!(prepare(&request).is_err());
}

#[test]
fn compose_version_does_not_require_compose_file() {
    let request = DockerCommandRequest {
        pty_id: None,
        cwd: "/home/sulion/repos/app".into(),
        argv: vec!["compose".into(), "--version".into()],
    };
    let prepared = prepare(&request).unwrap();
    assert_eq!(prepared.args, vec!["compose", "version"]);
}
