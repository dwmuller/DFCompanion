# UI improvements wish list

## Pure UI improvements

- Highlight start of user request vs AI response in conversation control.
  - Needs a better visual indicator.
- Might want more categories of notifications, perhaps explicitly shown in
  conversation window.
  - Particularly want to be able to distinguish "system" msgs, of which I want
    more, from actual back-and-forth with AI.
- Arrange various options as a top-of-page menu?
  - Sort of did this, but it's not beautiful yet.
- Arrange config mgmt list as a table.
  - Open forms will probably span all columns.
- Do some styling to get away from the drear black and white.

## Usability improvements for AI config

- Provide a toggle to turn on/off AI on main page. Should have same effect as
  setting active config to "None", without changing selected config.
- Move API key morphing button into edit/create forms.
  - Make API key optional per configuration with a toggle.
- Provide a method of editing global system text.
- Provide a per-config toggle to use global system text.
- Provide per-config system text.
  - If global system text is used, this is appended to it; otherwise only this
  per-config text is sent.
  - May be empty if not needed.