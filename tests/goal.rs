//! Shipped readers, corner commands, and Arroyo play commands on the GOG files.

use std::collections::HashSet;

use fallout2_3d::art::select_direction_art;
use fallout2_3d::camera::{pitch_degrees, yaw_degrees, CornerCamera, PITCH_DEG};
use fallout2_3d::dat2::GameFiles;
use fallout2_3d::direction::pick_direction;
use fallout2_3d::frm::{decode_frm, decode_palette, Palette};
use fallout2_3d::hexgrid::{self, hex_to_world, step};
use fallout2_3d::mapfile::load_game_map;
use fallout2_3d::occlusion::point_hidden;
use fallout2_3d::render::{self, RgbaImage, Rendered};
use fallout2_3d::scene::build_scene;
use fallout2_3d::{pick_direction as exported_pick, AttackKind, Command, Game};

#[test]
fn arroyo_readers_corners_and_play() {
    let files = GameFiles::open_default().expect("GOG archives");

    let palette_bytes = files.read("color.pal").expect("color.pal");
    println!("color.pal extracted {} bytes", palette_bytes.len());
    let palette = decode_palette(&palette_bytes).expect("palette");
    println!(
        "color.pal index 1 is [{}, {}, {}]",
        palette.rgb[1][0], palette.rgb[1][1], palette.rgb[1][2]
    );

    let map_bytes = files.read("maps\\arvillag.map").expect("arvillag");
    println!("maps\\arvillag.map extracted {} bytes", map_bytes.len());
    let version = i32::from_be_bytes(map_bytes[0..4].try_into().unwrap());
    let little = i32::from_le_bytes(map_bytes[0..4].try_into().unwrap());
    println!("maps\\arvillag.map version is {version} (little-endian misread {little})");
    assert_eq!(version, 20);
    assert_eq!(little, 335544320);

    let critter = files.read("art\\critters\\hmjmpsaa.frm").expect("hmjmpsaa");
    println!("art\\critters\\hmjmpsaa.frm extracted {} bytes", critter.len());
    let entry = files
        .critter
        .entry("art\\critters\\hmjmpsaa.frm")
        .expect("critter entry");
    println!(
        "hmjmpsaa.frm compressed {} uncompressed {} data {}",
        entry.compressed, entry.uncompressed_size, entry.data_size
    );
    assert!(entry.compressed, "hmjmpsaa.frm is a zlib entry");
    let frm = decode_frm(&critter).expect("hmjmpsaa frm");
    println!(
        "hmjmpsaa big-endian version {} direction offsets {:?} stored {:?}",
        frm.version, frm.direction_offsets, frm.stored_directions
    );
    assert_eq!(frm.version, 4);
    assert_eq!(frm.stored_directions.len(), 6);
    let mut distinct = frm.direction_offsets.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 6, "six distinct direction offsets");
    for direction in &frm.frames {
        for frame in direction {
            assert_eq!(frame.pixels.len(), frame.width as usize * frame.height as usize);
        }
    }
    println!(
        "hmjmpsaa frame pixels equal width*height (first {}x{})",
        frm.frames[0][0].width, frm.frames[0][0].height
    );

    let fr0_path = files
        .critter
        .paths()
        .find(|path| path.to_ascii_lowercase().ends_with(".fr0"))
        .expect("a .fr0")
        .to_string();
    let fr0 = decode_frm(&files.critter.read(&fr0_path).expect("fr0 bytes")).expect("fr0");
    println!(
        "{fr0_path} decodes as {} stored direction(s), version {}",
        fr0.stored_directions.len(),
        fr0.version
    );
    assert_eq!(fr0.stored_directions, vec![0]);
    assert_eq!(fr0.frames[0][0].transparent_index(), 0);
    println!("index 0 is flagged transparent");

    let help = "art\\intrface\\helpscrn.frm";
    let patched = files.read(help).expect("vfs helpscrn");
    let from_patch = files.patch.read(help).expect("patch helpscrn");
    let from_master = files.master.read(help).expect("master helpscrn");
    println!(
        "helpscrn.frm vfs {} patch {} master {}",
        patched.len(),
        from_patch.len(),
        from_master.len()
    );
    assert_eq!(patched, from_patch);
    assert_ne!(patched, from_master);
    println!("helpscrn.frm patch bytes differ from master");

    let game_map = load_game_map(&files, "maps\\arvillag.map").expect("load map");
    println!(
        "arvillag version {} bytes {} of {} name {}",
        game_map.version, game_map.bytes_consumed, game_map.file_size, game_map.name
    );
    assert_eq!(game_map.version, 20);
    assert_eq!(game_map.bytes_consumed, game_map.file_size);
    let level = game_map.populated();
    let mut walls = 0;
    let mut scenery = 0;
    let mut critters = 0;
    let mut items = 0;
    for object in &level.objects {
        assert!((0..40000).contains(&object.hex), "hex {}", object.hex);
        match object.kind {
            "wall" => walls += 1,
            "scenery" => scenery += 1,
            "critter" => critters += 1,
            "item" => items += 1,
            _ => {}
        }
    }
    let floors = level.floors.iter().filter(|cell| cell.art_index > 0).count();
    println!(
        "elevation {} floors {floors} walls {walls} scenery {scenery} items {items} critters {critters}",
        level.elevation
    );
    assert!(floors > 0 && walls > 0 && scenery > 0 && critters > 0);

    let art = fallout2_3d::art::ArtIndex::load(&files).expect("art");
    let scene = build_scene(&files, &art, &game_map).expect("scene");
    let critter_object = scene.critters().next().expect("scene critter");
    let critter_frm = decode_frm(&files.read(&critter_object.path).expect("critter art")).expect("decode");
    println!(
        "critter {} stored directions {:?}",
        critter_object.path, critter_frm.stored_directions
    );
    assert!(critter_frm.stored_directions.len() > 1);
    let (x0, z0) = hex_to_world(level.objects[0].hex);
    let (x1, z1) = hex_to_world(level.objects[1].hex);
    println!(
        "hex {} world ({x0}, {z0}) hex {} world ({x1}, {z1})",
        level.objects[0].hex, level.objects[1].hex
    );
    assert!(level.objects[0].hex != level.objects[1].hex);
    assert!(x0 != x1 || z0 != z1);

    let mut camera = CornerCamera::new(0);
    let mut yaws = Vec::new();
    for _ in 0..4 {
        let yaw = camera.yaw();
        let pitch = camera.pitch().expect("pitch");
        println!("corner {} yaw {yaw} pitch {pitch}", camera.corner);
        assert!((0.0..90.0).contains(&pitch));
        assert!((pitch - PITCH_DEG).abs() < 0.01);
        yaws.push(yaw);
        camera.advance();
    }
    assert_eq!(camera.corner, 0, "advance wraps");
    camera.back();
    assert_eq!(camera.corner, 3, "back wraps");
    camera.advance();
    for index in 0..4 {
        let next = yaws[(index + 1) % 4];
        let delta = (next - yaws[index]).rem_euclid(360.0);
        println!("yaw step {index} is {delta}");
        assert!((delta - 90.0).abs() < 0.01);
    }
    assert!((yaw_degrees(0) - 45.0).abs() < 0.01);
    assert!((pitch_degrees(2, PITCH_DEG).unwrap() - PITCH_DEG).abs() < 0.01);

    let six = [0, 1, 2, 3, 4, 5];
    println!(
        "pick k0 facing0 {} k2 {} k1 {} k3 {}",
        pick_direction(&six, 0, 0).unwrap(),
        pick_direction(&six, 0, 2).unwrap(),
        pick_direction(&six, 0, 1).unwrap(),
        pick_direction(&six, 0, 3).unwrap()
    );
    assert_eq!(exported_pick(&six, 0, 0).unwrap(), 0);
    assert_eq!(pick_direction(&six, 2, 0).unwrap(), 2);
    assert_eq!(pick_direction(&six, 2, 2).unwrap(), 5);
    assert_eq!(pick_direction(&six, 0, 2).unwrap(), 3);
    assert_eq!(pick_direction(&six, 0, 1).unwrap(), 5);
    assert_eq!(pick_direction(&six, 0, 3).unwrap(), 2);
    assert_eq!(pick_direction(&[4], 0, 3).unwrap(), 4);

    let (power_path, power_dir, power_multi) =
        select_direction_art(&files, "art\\critters\\hapowrba.fr0", 0, 0).expect("hapowrba");
    println!("hapowrba k0 facing0 {power_path} dir {power_dir} multi {power_multi}");
    assert!(power_path.to_ascii_lowercase().ends_with(".fr0"));
    assert!(power_multi);
    let (power_k2, _, _) = select_direction_art(&files, "art\\critters\\hapowrba.frm", 0, 2).unwrap();
    let (power_k1, _, _) = select_direction_art(&files, "art\\critters\\hapowrba.fr3", 0, 1).unwrap();
    let (power_k3, _, _) = select_direction_art(&files, "art\\critters\\hapowrba.fr1", 0, 3).unwrap();
    println!("hapowrba k2 {power_k2} k1 {power_k1} k3 {power_k3}");
    assert!(power_k2.to_ascii_lowercase().ends_with(".fr3"));
    assert!(power_k1.to_ascii_lowercase().ends_with(".fr5"));
    assert!(power_k3.to_ascii_lowercase().ends_with(".fr2"));

    let (floor_path, floor_dir, floor_multi) =
        select_direction_art(&files, "art\\tiles\\edg5000.frm", 3, 2).expect("floor");
    println!("edg5000 {floor_path} dir {floor_dir} multi {floor_multi}");
    assert!(!floor_multi);
    assert_eq!(floor_dir, 0);
    let floor = decode_frm(&files.read(&floor_path).unwrap()).unwrap();
    assert_eq!(floor.stored_directions.len(), 1);

    let look = fallout2_3d::camera::normalize([1.0, 0.0, 1.0]);
    let behind = [look[0] * 0.35, 1.0, look[2] * 0.35];
    let in_front = [-look[0] * 0.35, 1.0, -look[2] * 0.35];
    let beside = [-look[2] * 3.0, 1.0, look[0] * 3.0];
    println!(
        "occluder hides behind k0 {} k2 {} in_front {} beside {}",
        point_hidden(0, behind),
        point_hidden(2, behind),
        point_hidden(0, in_front),
        point_hidden(0, beside)
    );
    assert!(point_hidden(0, behind));
    assert!(!point_hidden(2, behind));
    assert!(!point_hidden(0, in_front));
    assert!(!point_hidden(0, beside));

    drop(scene);
    drop(game_map);
    drop(files);

    let mut game = Game::load().expect("playable arroyo");
    println!(
        "map enter ran {} procedures, player hex {} facing {}",
        game.entered.len(),
        game.player_tile(),
        game.player_facing()
    );
    assert_eq!(game.player_tile(), 20517);
    assert_eq!(game.player_facing(), 0);
    assert!(
        game.entered.iter().any(|name| name.ends_with(":map_enter_p_proc")),
        "map enter procedures: {:?}",
        game.entered
    );
    let map_script = game.script_name(game.map.map_script_index - 1);
    println!("map script {map_script} index {}", game.map.map_script_index - 1);
    assert!(map_script.eq_ignore_ascii_case("ArVillag"));

    let start = find_circuit(&game).expect("interior hex");
    println!("circuit start {start}");
    let ap_before_circuit = game.action_points();
    assert!(!game.in_combat(), "map enter is real time");
    game.place_player(start, 0);
    let mut seen = vec![start];
    for facing in 0..6 {
        let before = game.player_tile();
        let moved = game.command(Command::Step(facing)).expect("step").accepted;
        assert!(moved, "facing {facing} from {before} should be free");
        assert_eq!(game.player_facing(), facing);
        assert_ne!(game.player_tile(), before);
        seen.push(game.player_tile());
    }
    assert_eq!(
        game.action_points(),
        ap_before_circuit,
        "real-time steps do not spend action points"
    );
    println!("circuit hexes {seen:?}");
    assert_eq!(seen.len(), 7);
    assert_eq!(*seen.last().unwrap(), start);
    let mut unique = seen[..6].to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 6, "six neighbor steps are distinct");

    let (blocked_from, facing) = find_blocked(&game).expect("blocked neighbor");
    game.place_player(blocked_from, 0);
    let stayed = game.player_tile();
    let stayed_facing = game.player_facing();
    let stayed_ap = game.action_points();
    let accepted = game.command(Command::Step(facing)).expect("blocked step").accepted;
    println!(
        "blocked step from {blocked_from} facing {facing} accepted {accepted} hex {} facing {}",
        game.player_tile(),
        game.player_facing()
    );
    assert!(!accepted);
    assert_eq!(game.player_tile(), stayed);
    assert_eq!(game.player_facing(), stayed_facing);
    assert_eq!(game.action_points(), stayed_ap);

    game.place_player(start, 1);
    let free_before = game.player_tile();
    assert!(game.command(Command::Step(0)).unwrap().accepted);
    println!(
        "free step {free_before} -> {} facing {}",
        game.player_tile(),
        game.player_facing()
    );
    assert_ne!(game.player_tile(), free_before);
    assert_eq!(game.player_facing(), 0);

    let mut corners = Vec::new();
    game.camera = CornerCamera::new(0);
    for _ in 0..4 {
        corners.push(game.corner());
        game.advance_corner();
    }
    println!("advance corners {corners:?} then {}", game.corner());
    assert_eq!(corners, vec![0, 1, 2, 3]);
    assert_eq!(game.corner(), 0);
    game.back_corner();
    assert_eq!(game.corner(), 3);

    let looks = game.objects_with_proc("look_at_p_proc").expect("looks");
    let talks = game.objects_with_proc("talk_p_proc").expect("talks");
    let uses = game.objects_with_proc("use_p_proc").expect("uses");
    println!(
        "scripted look {} talk {} use {}",
        looks.len(),
        talks.len(),
        uses.len()
    );
    assert!(!looks.is_empty(), "a placed object has look_at_p_proc");
    assert!(!talks.is_empty(), "a placed object has talk_p_proc");
    assert!(!uses.is_empty(), "a placed object has use_p_proc");

    let look_target = &looks[0];
    let look = game.look(look_target.object_id).expect("look");
    println!(
        "look {} {} lines {:?} replies {:?} paths {:?}",
        look.script_name, look.procedure, look.lines, look.replies, look.message_paths
    );
    assert_eq!(look.procedure, "look_at_p_proc");
    assert!(
        game.entered.iter().any(|name| name.starts_with(&look.script_name) && name.ends_with("look_at_p_proc"))
    );

    let use_target = &uses[0];
    let used = game.use_object(use_target.object_id).expect("use");
    println!(
        "use {} {} lines {:?} replies {:?}",
        used.script_name, used.procedure, used.lines, used.replies
    );
    assert_eq!(used.procedure, "use_p_proc");

    let talk_target = talks
        .iter()
        .find(|object| object.script_index != look_target.script_index)
        .or(talks.first())
        .unwrap();
    let talk = game
        .command(Command::Talk(talk_target.object_id))
        .expect("talk")
        .dialogue
        .expect("talk dialogue");
    let talk_text = verbatim_line(&game, &talk).expect("talk text");
    println!(
        "talk {} {} reply {:?} text {:?} options {:?} paths {:?}",
        talk.script_name,
        talk.procedure,
        talk.replies.first(),
        talk_text,
        talk.options,
        talk.message_paths
    );
    assert_eq!(talk.procedure, "talk_p_proc");
    assert!(!talk_text.is_empty(), "talk text is empty");
    let msg_has_text = talk.message_paths.iter().any(|path| {
        String::from_utf8_lossy(&game.files.read(path).unwrap_or_default()).contains(&talk_text)
    });
    println!("talk text found in message file: {msg_has_text}");
    assert!(msg_has_text, "talk text must occur verbatim in the referenced message file");

    let other = talks
        .iter()
        .find(|object| object.script_index != talk_target.script_index)
        .expect("a second talk script");
    let other_talk = game
        .command(Command::Talk(other.object_id))
        .expect("second talk")
        .dialogue
        .expect("second dialogue");
    let other_text = verbatim_line(&game, &other_talk).expect("second talk text");
    println!(
        "second talk {} text {:?}",
        other_talk.script_name, other_text
    );
    assert_ne!(other_talk.script_name, talk.script_name);
    assert_ne!(other_text, talk_text);

    let swapped = game
        .run_swapped(talk_target.object_id, other.script_index, "talk_p_proc")
        .expect("swapped int");
    println!(
        "swapped raw {} replies {:?} options {:?} paths {:?}",
        swapped.script_name, swapped.replies, swapped.options, swapped.message_paths
    );
    let swapped_text = verbatim_line(&game, &swapped).expect("swapped text");
    println!(
        "swapped {} (was {}) text {:?}",
        swapped.script_name, talk.script_name, swapped_text
    );
    assert_ne!(swapped.script_name, talk.script_name);
    assert_ne!(swapped_text, talk_text);
    let stubs_before_combat: Vec<u16> = game.env.stub_ops.clone();
    println!(
        "stub opcodes {}",
        stubs_before_combat
            .iter()
            .map(|index| format!("{index:#x}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    let site = find_combat_site(&game).expect("open critter with a free approach");
    println!(
        "combat site critter {} hex {} origin {} stand {} approach {} retreat {}",
        site.critter_id, site.critter_hex, site.origin, site.stand, site.approach, site.retreat
    );
    assert_eq!(hexgrid::distance(site.stand, site.critter_hex), 1);
    assert_eq!(step(site.origin, site.approach).unwrap(), site.stand);

    let palette = decode_palette(&game.files.read("color.pal").unwrap()).unwrap();
    game.place_player(site.origin, site.approach);
    let before_step = render::render_corners(&mut game).expect("pre-step readback");
    println!("blade context: {}", before_step.device_name);
    let stepped = game.command(Command::Step(site.approach)).expect("render step").accepted;
    assert!(stepped, "approach hex is free");
    assert_eq!(game.player_tile(), site.stand);
    assert_eq!(game.player_facing(), site.approach);
    assert!(!game.in_combat());
    let after_step = render::render_corners(&mut game).expect("post-step readback");
    println!("texture readback: {}x{}", render::FRAME_WIDTH, render::FRAME_HEIGHT);
    assert_standing_move(&game, &palette, &before_step, &after_step, site.origin, site.stand);

    let report = play_combat(&mut game, &site);
    println!(
        "combat hex {} player hp {} target hp {} ap {} hit {}",
        report.hex, report.player_hp, report.target_hp, report.ap, report.hit
    );
    assert!(report.hit, "a resolved hit landed");
    assert!(game.in_combat(), "the hostile is still alive");
    assert_eq!(
        game.env
            .actors
            .iter()
            .find(|actor| actor.map_id == site.critter_id)
            .map(|actor| actor.tile),
        Some(site.critter_hex),
        "the adjacent hostile attacks in place"
    );

    let mut blows = 0;
    while game.is_alive_object(site.critter_id) {
        if game.action_points() < AttackKind::Melee.cost() {
            game.command(Command::EndTurn).unwrap();
            assert!(game.in_combat());
            assert_eq!(game.action_points(), game.max_action_points());
            assert_eq!(critter_tile(&game, site.critter_id), site.critter_hex);
        }
        let before_kill = game.hit_points(site.critter_id).unwrap();
        let killing = game
            .command(Command::Attack {
                object: site.critter_id,
                kind: AttackKind::Melee,
            })
            .unwrap()
            .strike
            .unwrap();
        println!(
            "killing blow refused {} hit {} damage {} hp {} -> {}",
            killing.refused,
            killing.hit,
            killing.damage,
            before_kill,
            game.hit_points(site.critter_id).unwrap()
        );
        assert!(!killing.refused && killing.hit && killing.damage > 0);
        assert_eq!(
            game.hit_points(site.critter_id).unwrap(),
            before_kill - killing.damage
        );
        blows += 1;
        assert!(blows < 6, "melee hits should drop the fixed critter pool");
    }
    assert!(!game.is_alive_object(site.critter_id));
    assert!(!game.in_combat(), "no living hostile remains");
    let ap_after_kill = game.action_points();
    assert_eq!(game.player_tile(), site.stand);

    let after_death = render::render_corners(&mut game).expect("post-death readback");
    assert_critter_removed(&game, &palette, &after_step, &after_death, &site);

    let corpse_facing = (0..6)
        .find(|facing| step(site.stand, *facing).ok() == Some(site.critter_hex))
        .expect("stand is adjacent to the corpse");
    assert!(
        !game.terrain_blocked(site.critter_hex),
        "the critter hex itself is not a wall"
    );
    assert!(
        game.command(Command::Step(corpse_facing)).unwrap().accepted,
        "a corpse does not block"
    );
    assert_eq!(game.player_tile(), site.critter_hex);
    assert_eq!(
        game.action_points(),
        ap_after_kill,
        "real-time steps after combat do not spend action points"
    );
    assert!(!game.in_combat());

    let doomed = find_combat_site(&game).expect("a second living critter");
    game.place_player(doomed.stand, doomed.approach);
    game.command(Command::StartCombat(doomed.critter_id)).unwrap();
    let mut rounds = 0;
    while game.player_alive() && rounds < 40 {
        game.command(Command::EndTurn).unwrap();
        rounds += 1;
    }
    println!(
        "player death after {rounds} hostile turns, hp {} alive {}",
        game.player_hit_points(),
        game.player_alive()
    );
    assert!(!game.player_alive(), "hostile turns reduce the player to 0 hp");
    assert_eq!(game.player_hit_points(), 0);
    let dead_hex = game.player_tile();
    let dead_facing = game.player_facing();
    assert!(
        !game.command(Command::Step(0)).unwrap().accepted,
        "player death refuses further steps"
    );
    assert_eq!(game.player_tile(), dead_hex);
    assert_eq!(game.player_facing(), dead_facing);

    let new_ops: Vec<u16> = game
        .env
        .stub_ops
        .iter()
        .copied()
        .filter(|op| !stubs_before_combat.contains(op))
        .collect();
    if new_ops.is_empty() {
        println!("combat path executed no new opcode");
    } else {
        println!(
            "combat opcodes {}",
            new_ops.iter().map(|index| format!("{index:#x}")).collect::<Vec<_>>().join(" ")
        );
    }

    let mut first = Game::load().expect("first combat run");
    let mut second = Game::load().expect("second combat run");
    let first_site = find_combat_site(&first).expect("site");
    let second_site = find_combat_site(&second).expect("site");
    assert_eq!(first_site.critter_id, second_site.critter_id);
    assert_eq!(first_site.stand, second_site.stand);
    let first_report = play_combat(&mut first, &first_site);
    let second_report = play_combat(&mut second, &second_site);
    println!(
        "second run hex {} hp {} target {} matches first {} {} {}",
        second_report.hex,
        second_report.player_hp,
        second_report.target_hp,
        first_report.hex,
        first_report.player_hp,
        first_report.target_hp
    );
    assert_eq!(first_report.hex, second_report.hex);
    assert_eq!(first_report.player_hp, second_report.player_hp);
    assert_eq!(first_report.target_hp, second_report.target_hp);
    assert_eq!(first_report.ap, second_report.ap);

    let mut played = Game::load().expect("dialogue session");
    let mut again = Game::load().expect("second dialogue session");
    let commands = fallout2_3d::session_commands(&mut played).expect("session commands");
    let mut opening = Vec::new();
    let mut lines = Vec::new();
    for command in &commands {
        let left = played.command(command.clone()).expect("session command");
        let right = again.command(command.clone()).expect("second session command");
        println!("{}", left.summary);
        assert_eq!(left.summary, right.summary, "the same choices yield the same words");
        if left.summary.starts_with("talk ") {
            let dialogue = left.dialogue.as_ref().expect("talk");
            opening = dialogue
                .replies
                .iter()
                .chain(dialogue.options.iter().map(|option| &option.text))
                .chain(dialogue.lines.iter())
                .cloned()
                .collect();
        }
        if left.summary.starts_with("chose option") {
            let dialogue = left.dialogue.as_ref().expect("choice");
            let fresh: Vec<String> = played
                .verbatim_lines(dialogue)
                .into_iter()
                .filter(|text| !opening.iter().any(|have| have == text))
                .collect();
            println!("choice fresh words {fresh:?}");
            assert!(!fresh.is_empty(), "the selected option must say something the opening did not");
            let words = left.summary.split("words ").nth(1).unwrap_or("");
            assert!(!words.is_empty(), "the log names the option's words");
            assert!(
                fresh.iter().any(|text| words.contains(text.as_str()) || text.contains(words)),
                "choice words must be verbatim from the option procedure"
            );
        }
        lines.push(left.summary);
    }
    assert!(
        lines.iter().any(|line| line.starts_with("played step 20517 ->")),
        "session steps off the map-enter hex"
    );
    assert!(
        lines.iter().any(|line| {
            line.starts_with("combat shot ")
                && (line.contains(" hit ") || line.contains(" miss "))
                && line.contains("spent ")
                && line.contains("target hp ")
        }),
        "session resolves a shot"
    );
    assert_eq!(played.player_tile(), again.player_tile());
    assert_eq!(played.player_hit_points(), again.player_hit_points());
}

fn verbatim_line(game: &Game, talk: &fallout2_3d::play::Interaction) -> Option<String> {
    let mut paths = talk.message_paths.clone();
    for path in [
        format!("text\\english\\dialog\\{}.msg", talk.script_name),
        format!("text\\english\\game\\{}.msg", talk.script_name),
    ] {
        if game.files.contains(&path) && !paths.iter().any(|have| have == &path) {
            paths.push(path);
        }
    }
    let blobs: Vec<String> = paths
        .iter()
        .map(|path| String::from_utf8_lossy(&game.files.read(path).unwrap_or_default()).into_owned())
        .collect();
    talk.replies
        .iter()
        .chain(talk.options.iter().map(|option| &option.text))
        .chain(talk.lines.iter())
        .find(|text| !text.is_empty() && blobs.iter().any(|blob| blob.contains(text.as_str())))
        .cloned()
}

fn find_circuit(game: &Game) -> Option<i32> {
    for tile in 0..40000 {
        if tile % 200 < 2 || tile % 200 > 197 || tile < 400 || tile > 39600 {
            continue;
        }
        if game.is_blocked(tile) {
            continue;
        }
        let mut cursor = tile;
        let mut seen = vec![tile];
        let mut open = true;
        for facing in 0..6 {
            cursor = match step(cursor, facing) {
                Ok(next) if (0..40000).contains(&next) && !game.is_blocked(next) => next,
                _ => {
                    open = false;
                    break;
                }
            };
            seen.push(cursor);
        }
        if open && cursor == tile && {
            let mut head = seen[..6].to_vec();
            head.sort_unstable();
            head.dedup();
            head.len() == 6
        } {
            return Some(tile);
        }
    }
    None
}

fn find_blocked(game: &Game) -> Option<(i32, i32)> {
    for tile in 0..40000 {
        if !game.terrain_blocked(tile) {
            continue;
        }
        for facing in 0..6 {
            let Ok(neighbor) = step(tile, facing) else { continue };
            if !(0..40000).contains(&neighbor) || game.terrain_blocked(neighbor) {
                continue;
            }
            let back = (facing + 3) % 6;
            if step(neighbor, back).ok() == Some(tile) {
                return Some((neighbor, back));
            }
        }
    }
    None
}

struct CombatSite {
    critter_id: i32,
    critter_hex: i32,
    origin: i32,
    stand: i32,
    approach: i32,
    retreat: i32,
}

struct CombatReport {
    hex: i32,
    player_hp: i32,
    target_hp: i32,
    ap: i32,
    hit: bool,
}

fn critter_tile(game: &Game, object_id: i32) -> i32 {
    game.env
        .actors
        .iter()
        .find(|actor| actor.handle > 1 && actor.map_id == object_id)
        .map(|actor| actor.tile)
        .unwrap_or(-1)
}

fn hex_open(game: &Game, tile: i32) -> bool {
    if !hexgrid::in_bounds(tile) || game.terrain_blocked(tile) {
        return false;
    }
    !game
        .env
        .actors
        .iter()
        .any(|actor| actor.handle > 1 && actor.alive && actor.kind == "critter" && actor.tile == tile)
}

fn find_combat_site(game: &Game) -> Option<CombatSite> {
    let mut best: Option<(i32, CombatSite)> = None;
    for actor in &game.env.actors {
        if actor.handle <= 1 || actor.kind != "critter" || !actor.alive {
            continue;
        }
        let critter_hex = actor.tile;
        for dir in 0..6 {
            let Ok(stand) = step(critter_hex, dir) else { continue };
            if !hex_open(game, stand) || hexgrid::distance(stand, critter_hex) != 1 {
                continue;
            }
            for away in 0..6 {
                let Ok(origin) = step(stand, away) else { continue };
                let approach = (away + 3) % 6;
                if step(origin, approach).ok() != Some(stand) || origin == critter_hex || !hex_open(game, origin)
                {
                    continue;
                }
                let walls = (0..6)
                    .filter(|facing| {
                        step(critter_hex, *facing)
                            .ok()
                            .is_some_and(|next| game.terrain_blocked(next))
                    })
                    .count() as i32;
                let site = CombatSite {
                    critter_id: actor.map_id,
                    critter_hex,
                    origin,
                    stand,
                    approach,
                    retreat: away,
                };
                let replace = best.as_ref().map(|(score, _)| walls < *score).unwrap_or(true);
                if replace {
                    best = Some((walls, site));
                    if walls == 0 {
                        return best.map(|(_, found)| found);
                    }
                }
            }
        }
    }
    best.map(|(_, found)| found)
}

fn play_combat(game: &mut Game, site: &CombatSite) -> CombatReport {
    let max_ap = game.max_action_points();
    let agility = game.env.base_stat(5);
    println!("max ap {max_ap} = 5 + agility {agility} / 2");
    assert_eq!(max_ap, 5 + agility / 2);
    game.place_player(site.stand, site.approach);
    assert!(!game.in_combat());
    game.command(Command::StartCombat(site.critter_id)).expect("start combat");
    assert!(game.in_combat());
    assert_eq!(game.action_points(), max_ap);

    let toward = hexgrid::rotation_to(site.stand, site.critter_hex);
    let blocked_facing = game.player_facing();
    assert!(
        !game.command(Command::Step(toward)).unwrap().accepted,
        "a living critter blocks the step"
    );
    assert_eq!(game.player_tile(), site.stand);
    assert_eq!(game.player_facing(), blocked_facing);
    assert_eq!(game.action_points(), max_ap);
    if let Some(wall_facing) = (0..6).find(|facing| {
        step(site.stand, *facing)
            .ok()
            .is_some_and(|next| game.terrain_blocked(next))
    }) {
        assert!(!game.command(Command::Step(wall_facing)).unwrap().accepted);
        assert_eq!(game.player_tile(), site.stand);
        assert_eq!(game.player_facing(), blocked_facing);
        assert_eq!(game.action_points(), max_ap);
    }

    let hp_before_shot = game.hit_points(site.critter_id).unwrap();
    let shot = game
        .command(Command::Attack {
            object: site.critter_id,
            kind: AttackKind::Shot,
        })
        .unwrap()
        .strike
        .unwrap();
    println!(
        "shot refused {} hit {} damage {} spent {} ap {}",
        shot.refused, shot.hit, shot.damage, shot.spent, game.action_points()
    );
    assert!(!shot.refused);
    assert!(!shot.hit);
    assert_eq!(shot.damage, 0);
    assert_eq!(shot.spent, AttackKind::Shot.cost());
    assert_eq!(game.hit_points(site.critter_id).unwrap(), hp_before_shot);
    assert_eq!(game.action_points(), max_ap - AttackKind::Shot.cost());

    let refused = game
        .command(Command::Attack {
            object: site.critter_id,
            kind: AttackKind::Unarmed,
        })
        .unwrap()
        .strike
        .unwrap();
    println!(
        "unarmed refused {} spent {} ap {}",
        refused.refused, refused.spent, game.action_points()
    );
    assert!(refused.refused);
    assert_eq!(refused.spent, 0);
    assert_eq!(refused.damage, 0);
    assert!(!refused.hit);
    assert_eq!(game.hit_points(site.critter_id).unwrap(), hp_before_shot);
    assert_eq!(game.action_points(), max_ap - AttackKind::Shot.cost());

    let leftover = game.action_points();
    assert!(leftover > 0, "the refused attack leaves points in the pool");
    let player_hp_before_turn = game.player_hit_points();
    game.command(Command::EndTurn).unwrap();
    let hostile_damage = player_hp_before_turn - game.player_hit_points();
    println!(
        "end turn hostile damage {hostile_damage} player hp {} ap {} leftover was {leftover}",
        game.player_hit_points(),
        game.action_points()
    );
    println!(
        "refill sets ap to {} and does not bank leftover {leftover} (banked would be {})",
        game.action_points(),
        max_ap + leftover
    );
    assert!(game.in_combat());
    assert!(hostile_damage > 0, "the hostile's turn connects");
    assert_eq!(critter_tile(game, site.critter_id), site.critter_hex);
    assert_eq!(game.action_points(), max_ap);
    assert_ne!(game.action_points(), max_ap + leftover, "leftover points are not banked");

    let before_melee = game.hit_points(site.critter_id).unwrap();
    let melee = game
        .command(Command::Attack {
            object: site.critter_id,
            kind: AttackKind::Melee,
        })
        .unwrap()
        .strike
        .unwrap();
    println!(
        "melee refused {} hit {} damage {} spent {} hp {}",
        melee.refused,
        melee.hit,
        melee.damage,
        melee.spent,
        game.hit_points(site.critter_id).unwrap()
    );
    assert!(!melee.refused && melee.hit && melee.damage > 0);
    assert_eq!(melee.spent, AttackKind::Melee.cost());
    assert_eq!(game.hit_points(site.critter_id).unwrap(), before_melee - melee.damage);
    assert_eq!(game.action_points(), max_ap - AttackKind::Melee.cost());

    let before_unarmed = game.hit_points(site.critter_id).unwrap();
    let unarmed = game
        .command(Command::Attack {
            object: site.critter_id,
            kind: AttackKind::Unarmed,
        })
        .unwrap()
        .strike
        .unwrap();
    println!(
        "unarmed hit {} damage {} spent {} hp {}",
        unarmed.hit,
        unarmed.damage,
        unarmed.spent,
        game.hit_points(site.critter_id).unwrap()
    );
    assert!(!unarmed.refused && unarmed.hit && unarmed.damage > 0);
    assert_eq!(unarmed.spent, AttackKind::Unarmed.cost());
    assert_eq!(
        game.hit_points(site.critter_id).unwrap(),
        before_unarmed - unarmed.damage
    );
    assert!(
        game.hit_points(site.critter_id).unwrap() > 0,
        "the compared run leaves the hostile alive"
    );

    game.command(Command::EndTurn).unwrap();
    assert_eq!(game.action_points(), max_ap);
    assert_eq!(critter_tile(game, site.critter_id), site.critter_hex);
    assert!(
        game.command(Command::Step(site.retreat)).unwrap().accepted,
        "a combat step costs one point"
    );
    assert_eq!(game.player_tile(), site.origin);
    assert_eq!(game.player_facing(), site.retreat);
    assert_eq!(game.action_points(), max_ap - 1);
    assert!(
        game.command(Command::Step(site.approach)).unwrap().accepted,
        "step back into reach"
    );
    assert_eq!(game.player_tile(), site.stand);
    assert_eq!(game.action_points(), max_ap - 2);

    CombatReport {
        hex: game.player_tile(),
        player_hp: game.player_hit_points(),
        target_hp: game.hit_points(site.critter_id).unwrap(),
        ap: game.action_points(),
        hit: true,
    }
}

fn assert_standing_move(
    game: &Game,
    palette: &Palette,
    before: &Rendered,
    after: &Rendered,
    origin: i32,
    destination: i32,
) {
    assert_eq!(before.frames.len(), 4);
    assert_eq!(after.frames.len(), 4);
    for corner in 0..4 {
        let earlier = &before.frames[corner];
        let later = &after.frames[corner];
        let corner_i = corner as i32;
        assert_eq!((later.width, later.height), (render::FRAME_WIDTH, render::FRAME_HEIGHT));
        assert_eq!((earlier.width, earlier.height), (render::FRAME_WIDTH, render::FRAME_HEIGHT));
        let fill = painted_fraction(later);
        println!("corner {corner} painted fraction {fill:.3}");
        assert!(fill >= 0.6, "corner {corner} fill {fill:.3} is below 0.6");
        if corner > 0 {
            assert_ne!(
                after.frames[corner - 1].pixels,
                later.pixels,
                "corner {corner} matches the previous corner"
            );
        }
        let colors = standing_colors(
            game,
            palette,
            "art\\critters\\hmjmpsaa.frm",
            game.player_facing(),
            corner_i,
        );
        let (appeared, changed) = sprite_delta(game, earlier, later, corner_i, destination, origin, &colors, true);
        let (left, origin_changed) =
            sprite_delta(game, earlier, later, corner_i, origin, destination, &colors, false);
        println!(
            "corner {corner} destination appeared {appeared} changed {changed} origin left {left} changed {origin_changed}"
        );
        assert!(
            appeared > 0,
            "corner {corner} does not show the standing art on the new hex"
        );
        assert!(
            left > 0,
            "corner {corner} still shows the standing art only as if the step had not happened"
        );
    }
}

fn assert_critter_removed(
    game: &Game,
    palette: &Palette,
    alive: &Rendered,
    dead: &Rendered,
    site: &CombatSite,
) {
    let object = game
        .scene
        .objects
        .iter()
        .find(|object| object.id == site.critter_id)
        .expect("critter art");
    for corner in 0..4 {
        let corner_i = corner as i32;
        let colors = standing_colors(game, palette, &object.path, object.facing, corner_i);
        let (gone, changed) = sprite_delta(
            game,
            &alive.frames[corner],
            &dead.frames[corner],
            corner_i,
            site.critter_hex,
            site.stand,
            &colors,
            false,
        );
        println!("corner {corner} critter standing pixels removed {gone} changed {changed}");
        assert!(gone > 0, "corner {corner} still draws the dead critter standing");
        let fill = painted_fraction(&dead.frames[corner]);
        assert!(fill >= 0.6, "post-death corner {corner} fill {fill:.3}");
    }
}

fn standing_colors(
    game: &Game,
    palette: &Palette,
    path: &str,
    facing: i32,
    corner: i32,
) -> HashSet<(u8, u8, u8)> {
    let (art_path, direction, _) = select_direction_art(&game.files, path, facing, corner).expect("art");
    let frm = decode_frm(&game.files.read(&art_path).unwrap()).unwrap();
    let frame = frm.direction_frame(direction).unwrap();
    let mut colors = HashSet::new();
    for &index in &frame.pixels {
        if index == 0 {
            continue;
        }
        let rgb = palette.rgb[index as usize];
        colors.insert((rgb[0], rgb[1], rgb[2]));
    }
    colors
}

fn sprite_box(game: &Game, corner: i32, tile: i32, other: i32) -> (i32, i32, i32, i32) {
    let (x, z) = hex_to_world(tile);
    let (ox, oz) = hex_to_world(other);
    let (qx, qy) = render::project_point(game, corner, [ox, 1.0, oz]);
    let (px, py) = render::project_point(game, corner, [x, 0.9, z]);
    let spacing = (px - qx).hypot(py - qy);
    let pad = (spacing * 0.3).clamp(3.0, 10.0);
    println!(
        "corner {corner} project tile {tile} body ({px:.1}, {py:.1}) spacing {spacing:.1} pad {pad:.1}"
    );
    assert!(
        (0.0..render::FRAME_WIDTH as f32).contains(&px) && (0.0..render::FRAME_HEIGHT as f32).contains(&py),
        "projected hex {tile} is outside the frame at ({px:.1}, {py:.1})"
    );
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for height in [0.15_f32, 0.45, 0.8, 1.15, 1.6, 2.0] {
        let (sx, sy) = render::project_point(game, corner, [x, height, z]);
        let left = (sx - pad).floor() as i32;
        let right = (sx + pad).ceil() as i32;
        let top = (sy - pad).floor() as i32;
        let bottom = (sy + pad).ceil() as i32;
        min_x = min_x.min(left);
        max_x = max_x.max(right);
        min_y = min_y.min(top);
        max_y = max_y.max(bottom);
    }
    (min_x, min_y, max_x, max_y)
}

fn sprite_delta(
    game: &Game,
    before: &RgbaImage,
    after: &RgbaImage,
    corner: i32,
    tile: i32,
    other: i32,
    colors: &HashSet<(u8, u8, u8)>,
    arriving: bool,
) -> (usize, usize) {
    let (min_x, min_y, max_x, max_y) = sprite_box(game, corner, tile, other);
    let width = after.width as i32;
    let height = after.height as i32;
    let mut matched = 0usize;
    let mut changed = 0usize;
    for y in min_y.max(0)..=max_y.min(height - 1) {
        for x in min_x.max(0)..=max_x.min(width - 1) {
            let index = ((y as u32 * after.width + x as u32) * 4) as usize;
            let earlier = (
                before.pixels[index],
                before.pixels[index + 1],
                before.pixels[index + 2],
            );
            let later = (
                after.pixels[index],
                after.pixels[index + 1],
                after.pixels[index + 2],
            );
            if earlier == later {
                continue;
            }
            changed += 1;
            let sample = if arriving { later } else { earlier };
            if colors.contains(&sample) {
                matched += 1;
            }
        }
    }
    (matched, changed)
}

fn painted_fraction(image: &RgbaImage) -> f32 {
    let mut painted = 0u32;
    for pixel in image.pixels.chunks(4) {
        if pixel[0] as u16 + pixel[1] as u16 + pixel[2] as u16 > 24 {
            painted += 1;
        }
    }
    painted as f32 / (image.width * image.height) as f32
}
