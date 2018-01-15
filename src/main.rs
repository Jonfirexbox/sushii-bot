#![recursion_limit="256"]

#[macro_use]
extern crate log;

#[macro_use]
extern crate serenity;

#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate serde_json;

#[macro_use]
extern crate diesel;
extern crate r2d2;
extern crate r2d2_diesel;

#[macro_use]
extern crate diesel_migrations;

#[macro_use]
extern crate lazy_static;

extern crate dotenv;
extern crate env_logger;
extern crate reqwest;
extern crate typemap;
extern crate chrono;
extern crate chrono_humanize;
extern crate rand;
extern crate inflector;
extern crate regex;
extern crate darksky;
extern crate tzdata;

pub mod schema;
pub mod models;
#[macro_use]
pub mod utils;

#[macro_use]
mod plugins;
mod commands;
mod tasks;
mod handler;
mod database;

use serenity::framework::StandardFramework;
use serenity::framework::standard::help_commands;
use serenity::framework::standard::DispatchError::*;

use serenity::model::permissions::Permissions;
use serenity::model::UserId;
use serenity::prelude::*;

use std::collections::HashSet;
use std::env;
use dotenv::dotenv;

use typemap::Key;
use database::ConnectionPool;

impl Key for ConnectionPool {
    type Value = ConnectionPool;
}

embed_migrations!("./migrations");

fn main() {
    dotenv().ok();

    // Initialize the logger to use environment variables.
    //
    // In this case, a good default is setting the environment variable
    // `RUST_LOG` to debug`.
    env_logger::init().expect("Failed to initialize env_logger");

    let mut client =
        Client::new(
            &env::var("DISCORD_TOKEN").expect("Expected a discord token in the environment."),
            handler::Handler,
        );

    {
        let mut data = client.data.lock();
        let pool = database::init();

        data.insert::<ConnectionPool>(pool);
    }

    let owners: HashSet<UserId> = env::var("OWNER")
        .expect("Expected owner IDs in the environment.")
        .split(",")
        .map(|x| UserId(x.parse::<u64>().unwrap()))
        .collect();

    let blocked_users: HashSet<UserId> = match env::var("BLOCKED_USERS") {
        Ok(val) => {
            val.split(",").map(|x| UserId(x.parse::<u64>().unwrap())).collect()
        },
        Err(_) => HashSet::new(),
    };


    client.with_framework(
        StandardFramework::new()
            .configure(|c| c
                .owners(owners)
                .dynamic_prefix(|ctx, msg| {
                    let mut data = ctx.data.lock();
                    let pool = data.get_mut::<database::ConnectionPool>().unwrap();

                    // get guild id
                    if let Some(guild_id) = msg.guild_id() {
                        // get guild config prefix
                        if let Some(prefix) = pool.get_prefix(guild_id.0) {
                            return Some(prefix);
                        }
                    }

                    // either no guild found or no prefix set for guild, use default
                    let default_prefix = env::var("DEFAULT_PREFIX").expect("Expected DEFAULT_PREFIX in the environment.");
                    Some(default_prefix)
                })
                .blocked_users(blocked_users)
                .allow_whitespace(true)
            )
            .on_dispatch_error(|_, msg, error| {
                let mut s = String::new();
                match error {
                    NotEnoughArguments { min, given } => {
                        s = format!("Need {} arguments, but only got {}.", min, given);
                    }
                    TooManyArguments { max, given } => {
                        s = format!("Too many arguments, need {}, but got {}.", max, given);
                    }
                    LackOfPermissions(permissions) => {
                        s = format!(
                            "You do not have permission for this command.  Requires `{:?}`.",
                            permissions
                        );
                    }
                    OnlyForOwners => {
                        s = "no.".to_owned();
                    }
                    OnlyForGuilds => {
                        s = "This command can only be used in guilds.".to_owned();
                    }
                    RateLimited(seconds) => {
                        s = format!("Try this again in {} seconds.", seconds);
                    }
                    BlockedUser => {
                        println!("Blocked user {} attemped to use command.", msg.author.tag());
                    }
                    _ => println!("Unhandled dispatch error."),
                }

                let _ = msg.channel_id.say(&s);

                // react x whenever an error occurs
                let _ = msg.react("❌");
            })
            .before(|_ctx, msg, cmd_name| {
                println!("{}: {} ", msg.author.tag(), cmd_name);
                true
            })
            .after(|_ctx, msg, cmd_name, error| {
                //  Print out an error if it happened
                if let Err(why) = error {
                    // react x whenever an error occurs
                    let _ = msg.react("❌");
                    let s = format!("Error: {}", why.0);

                    let _ = msg.channel_id.say(&s);
                    println!("Error in {}: {:?}", cmd_name, why);
                }
            })
            .simple_bucket("rank_bucket", 15)
            .group("Ranking", |g| {
                g.bucket("rank_bucket")
                    .guild_only(true)
                    .command("rank", |c| {
                        c.desc("Shows your current rank.")
                        .exec(commands::levels::rank)
                    })
                    .command("rep", |c| {
                        c.desc("Rep a user.")
                        .exec(commands::levels::rep)
                    })
            })
            .group("Notifications", |g| {
                g.prefix("notification")
                    .command("add", |c| {
                        c.desc("Adds a notification.")
                            .exec(commands::notifications::add_notification)
                    })
                    .command("list", |c| {
                        c.desc("Lists your set notifications")
                            .exec(commands::notifications::list_notifications)
                    })
                    .command("delete", |c| {
                        c.desc("Deletes a notification")
                            .exec(commands::notifications::delete_notification)
                    })
            })
            .group("Meta", |g| {
                g.command("help", |c| c.exec_help(help_commands::with_embeds))
                    .command("helpp", |c| c.exec_help(help_commands::plain))
                    .command("ping", |c| c.exec_str("Pong!"))
                    .command("latency", |c| {
                        c.desc(
                            "Calculates the heartbeat latency between the shard and the gateway.",
                        ).exec(commands::meta::latency)
                    })
                    .command("events", |c| {
                        c.desc("Shows the number of events handled by the bot.")
                            .exec(commands::meta::events)
                    })
            })
            .group("Moderation", |g| {
                g.command("reason", |c| {
                        c.desc("Edits the reason for moderation action cases.")
                        .required_permissions(Permissions::MANAGE_GUILD)
                        .exec(commands::moderation::cases::reason)
                    })
                    .command("ban", |c| {
                        c.usage("[mention or id](,mention or id) [reason]")
                        .desc("Bans a user or ID.")
                        .required_permissions(Permissions::BAN_MEMBERS)
                        .exec(commands::moderation::ban::ban)
                    })
                    .command("mute", |c| {
                        c.usage("[mention or id]")
                        .desc("Mutes a member.")
                        .required_permissions(Permissions::BAN_MEMBERS)
                        .exec(commands::moderation::mute::mute)
                    })
            })
            .group("Settings", |g| {
                g.guild_only(true)
                    .command("prefix", |c| {
                    c.desc("Gives you the prefix for this guild, or sets a new prefix (Setting prefix requires MANAGE_GUILD).")
                        .exec(commands::settings::prefix)
                    })
                    .command("joinmsg", |c| {
                        c.desc("Gets the guild's join message or sets one if given.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::joinmsg)
                    })
                    .command("leavemsg", |c| {
                        c.desc("Gets the guild's leave message or sets one if given.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::leavemsg)
                    })
                    .command("modlog", |c| {
                        c.desc("Sets the moderation log channel.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::modlog)
                    })
                    .command("msglog", |c| {
                        c.desc("Sets the message log channel.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::msglog)
                    })
                    .command("memberlog", |c| {
                        c.desc("Sets the member log channel.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::memberlog)
                    })
                    .command("inviteguard", |c| {
                        c.desc("Enables or disables the invite guard.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::inviteguard)
                    })
                    .command("muterole", |c| {
                        c.desc("Sets the mute role.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::mute_role)
                    })
                    .command("maxmentions", |c| {
                        c.desc("Sets the maximum mentions a user can have in a single message before automatically being muted.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::max_mentions)
                    })
            })
            .group("Roles", |g| {
                g.prefix("roles")
                    .guild_only(true)
                    .required_permissions(Permissions::MANAGE_GUILD)
                    .command("set", |c| {
                        c.desc("Sets the role configuration.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::roles_set)
                    })
                    .command("get", |c| {
                        c.desc("Gets the role configuration.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::roles_get)
                    })
                    .command("channel", |c| {
                        c.desc("Sets the roles channel.")
                            .required_permissions(Permissions::MANAGE_GUILD)
                            .exec(commands::settings::roles_channel)
                    })
            })
            .group("Misc", |g| {
                g.command("play", |c| {
                    c.usage("[rust code]")
                        .desc("Evaluates Rust code in the playground.")
                        .min_args(1)
                        .exec(commands::misc::play)
                })
                .command("reminder", |c| {
                    c.usage("[time] [description]")
                        .desc("Reminds you to do something after some time.")
                        .exec(commands::misc::reminder)
                })
                .command("reminders", |c| {
                    c.desc("Shows your pending reminders.")
                        .exec(commands::misc::reminders)
                })
                .command("crypto", |c| {
                    c.usage("(symbol)")
                        .desc("Gets current cryptocurrency prices.")
                        .exec(commands::crypto::crypto)
                })
            })
            .group("Search", |g| {
                g.command("weather", |c| {
                    c.usage("[location]")
                        .desc("Gets the weather of a location")
                        .exec(commands::search::weather::weather)
                })
            })
            .group("User Info", |g| {
                g.command("userinfo", |c| {
                    c.usage("[user]")
                        .desc("Gets information about a user.")
                        .exec(commands::userinfo::userinfo)
                })
                .command("avatar", |c| {
                    c.usage("[user]")
                        .desc("Gets the avatar for a user.")
                        .exec(commands::userinfo::avatar)
                })
            })
            .group("Owner", |g| {
                g.command("quit", |c| {
                    c.desc("Gracefully shuts down the bot.")
                        .owners_only(true)
                        .exec(commands::owner::quit)
                        .known_as("shutdown")
                }).command("reset events", |c| {
                        c.desc("Resets the events counter.")
                            .owners_only(true)
                            .exec(commands::meta::reset_events)
                    })
                    .command("username", |c| {
                        c.desc("Changes the bot's username.")
                            .owners_only(true)
                            .exec(commands::owner::username)
                    })
            }),
    );

    if let Err(why) = client.start() {
        error!("Client error: {:?}", why);
    }
}
